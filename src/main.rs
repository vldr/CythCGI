extern crate fastcgi;

use std::{
    alloc::Layout,
    cmp,
    collections::HashMap,
    env::{self, args},
    ffi::{CStr, CString, c_char, c_float, c_int, c_void},
    fmt::Write as _,
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::ExitCode,
    ptr::{self, null},
    rc::Rc,
    slice,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bcrypt::DEFAULT_COST;
use chrono::{DateTime, Local};
use markdown::{CompileOptions, Options};
use percent_encoding::NON_ALPHANUMERIC;

use fastcgi::Request;
use sqlite::{Connection, ConnectionThreadSafe, State, Statement, Value};
use uuid::Uuid;

struct Script {
    modified: SystemTime,
    text: Rc<Vec<String>>,
    mapping: Rc<Vec<(i32, i32)>>,
    functions: Functions,
    vm: *const c_void,
}

#[derive(Default)]
struct Context {
    input: String,
    output: String,
    headers: String,
    text: Rc<Vec<String>>,
    environs: Rc<HashMap<String, String>>,
    connections: Vec<ConnectionThreadSafe>,
    statements: Vec<Statement>,
    mapping: Rc<Vec<(i32, i32)>>,
    path: String,
    functions: Functions,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct Functions {
    json_number: unsafe extern "C" fn(number: f32) -> *const c_void,
    json_bool: unsafe extern "C" fn(bool: c_int) -> *const c_void,
    json_string: unsafe extern "C" fn(string: *const CyString) -> *const c_void,
    json_array: unsafe extern "C" fn(array: *const CyArray<*const c_void>) -> *const c_void,
    json_object: unsafe extern "C" fn(map: *const c_void) -> *const c_void,
    map_init: unsafe extern "C" fn(this: *const c_void) -> *const c_void,
    map_set: unsafe extern "C" fn(this: *const c_void, key: *const CyString, value: *const c_void),
}

impl Default for Functions {
    fn default() -> Self {
        unsafe { std::mem::transmute::<[usize; 7], Functions>([0; 7]) }
    }
}

#[repr(C)]
pub struct CyString {
    pub size: i32,
    pub data: [u8; 0],
}

#[repr(C)]
pub struct CyArray<T> {
    pub size: i32,
    pub capacity: i32,
    pub data: *mut T,
}

unsafe extern "C" {
    fn cyth_init() -> *const c_void;
    fn cyth_set_error_callback(vm: *const c_void, error_callback: *const c_void);
    fn cyth_set_panic_callback(vm: *const c_void, panic_callback: *const c_void);
    fn cyth_load_function(
        vm: *const c_void,
        signature: *const c_char,
        func: *const c_void,
    ) -> c_int;
    fn cyth_load_string(vm: *const c_void, filename: *const c_char, source: *const c_char)
    -> c_int;
    fn cyth_get_function(vm: *const c_void, name: *const c_char) -> *const c_void;
    fn cyth_compile(vm: *const c_void) -> c_int;
    fn cyth_run(vm: *const c_void);
    fn cyth_destroy(vm: *const c_void);
    fn cyth_alloc(atomic: c_int, size: usize) -> *const c_void;
}

fn cyth_new_string(string: &str) -> *mut CyString {
    unsafe {
        let layout = Layout::from_size_align(
            std::mem::size_of::<CyString>() + string.len(),
            std::mem::align_of::<CyString>(),
        )
        .unwrap();

        let cyth_string = cyth_alloc(1, layout.size()) as *mut CyString;
        if cyth_string.is_null() {
            std::alloc::handle_alloc_error(layout);
        }

        (*cyth_string).size = string.len() as i32;
        ptr::copy_nonoverlapping(
            string.as_ptr(),
            (*cyth_string).data.as_mut_ptr(),
            string.len(),
        );

        cyth_string
    }
}

fn cyth_new_array<T: Copy>(list: impl ExactSizeIterator<Item = T>) -> *const CyArray<T> {
    unsafe {
        let data_size = list.len();
        let data_atomic = !std::any::type_name::<T>().starts_with("*") as i32;
        let data_layout = Layout::array::<T>(data_size).unwrap();
        let data_ptr = cyth_alloc(data_atomic, data_layout.size()) as *mut T;
        if data_ptr.is_null() {
            std::alloc::handle_alloc_error(data_layout);
        }

        for (index, item) in list.enumerate() {
            ptr::write(data_ptr.add(index), item);
        }

        let cyth_array_layout = Layout::new::<CyArray<*mut T>>();
        let cyth_array = cyth_alloc(0, cyth_array_layout.size()) as *mut CyArray<T>;
        if cyth_array.is_null() {
            std::alloc::handle_alloc_error(cyth_array_layout);
        }

        (*cyth_array).size = data_size as i32;
        (*cyth_array).capacity = data_size as i32;
        (*cyth_array).data = data_ptr;

        cyth_array
    }
}

fn cyth_new_json(value: &serde_json::Value) -> *const c_void {
    let context = unsafe { &mut *CONTEXT };

    match value {
        serde_json::Value::Null => null(),
        serde_json::Value::Bool(value) => unsafe { (context.functions.json_bool)(*value as i32) },
        serde_json::Value::Number(value) => unsafe {
            (context.functions.json_number)(value.as_f64().unwrap() as f32)
        },
        serde_json::Value::String(value) => unsafe {
            (context.functions.json_string)(cyth_new_string(value))
        },
        serde_json::Value::Array(values) => unsafe {
            (context.functions.json_array)(cyth_new_array(
                values.iter().map(|value| cyth_new_json(value)),
            ))
        },
        serde_json::Value::Object(map) => unsafe {
            let this = (context.functions.map_init)(null());

            for (key, value) in map {
                (context.functions.map_set)(this, cyth_new_string(key), cyth_new_json(value));
            }

            (context.functions.json_object)(this)
        },
    }
}

fn cyth_string_to_str<'a>(cyth_string: *const CyString) -> &'a str {
    unsafe {
        let slice =
            slice::from_raw_parts((*cyth_string).data.as_ptr(), (*cyth_string).size as usize);
        str::from_utf8_unchecked(slice)
    }
}

fn cyth_char_array_to_slice<'a>(cyth_array: *const CyArray<u8>) -> &'a [u8] {
    unsafe { slice::from_raw_parts((*cyth_array).data, (*cyth_array).size as usize) }
}

extern "C" fn error_callback(
    filename: *const c_char,
    start_line: c_int,
    start_column: c_int,
    end_line: c_int,
    end_column: c_int,
    message: *const c_char,
) {
    let context = unsafe { &mut *CONTEXT };
    let message = unsafe { CStr::from_ptr(message).to_str().unwrap_or("") };
    let filename = unsafe { CStr::from_ptr(filename).to_str().unwrap_or("") };

    if filename == context.path {
        let (mapped_start_line, mapped_start_column) = context
            .mapping
            .get((start_line - 1) as usize)
            .copied()
            .unwrap_or((start_line, 0));

        let (mapped_end_line, mapped_end_column) = context
            .mapping
            .get((end_line - 1) as usize)
            .copied()
            .unwrap_or((end_line, 0));

        context.output.push_str(&format!(
            "{}:{}:{}-{}:{}: {}\n",
            filename,
            mapped_start_line,
            mapped_start_column + start_column,
            mapped_end_line,
            mapped_end_column + end_column,
            message
        ));
    } else {
        context.output.push_str(&format!(
            "{}:{}:{}-{}:{}: {}\n",
            filename, start_line, start_column, end_line, end_column, message
        ));
    }
}

extern "C" fn panic_callback(function: *const c_char, line: c_int, column: c_int) {
    let context = unsafe { &mut *CONTEXT };

    if line == 0 && column == 0 {
        context.headers.clear();
        context
            .headers
            .push_str("Status: 500 Internal Server Error\n");
        context.headers.push_str("Content-Type: text/plain\n");

        context.output.push_str(&format!("{}\n", unsafe {
            CStr::from_ptr(function).to_str().unwrap()
        }));
    } else {
        let (mapped_line, mapped_column) = context
            .mapping
            .get((line - 1) as usize)
            .copied()
            .unwrap_or((line, 0));

        context.output.push_str(&format!(
            "  at {}:{}:{}\n",
            unsafe { CStr::from_ptr(function).to_str().unwrap() },
            mapped_line,
            mapped_column + column,
        ));
    }
}

const BUILTINS: &str = r#"
Map<string, string> parseQuery(string query)
    Map<string, string> result = Map<string, string>()

    string[] pairs = query.split("&")
    for string pair in pairs
        string[] parts = pair.split("=")

        if parts.length == 2
            result.insert(parts[0], urlDecode(parts[1]))

    return result

class Database
    int con

    void __init__(string path)
        this.con = sqliteOpen(path)

    Statement prepare(string query)
        int stmt = sqlitePrepare(con, query)
        if stmt
            return Statement(stmt)

        return null

    bool execute(string a)
        return sqliteExecute(con, a)

class Statement
    int stmt

    void __init__(int stmt)
        this.stmt = stmt

    int readInt(string column)
        return sqliteReadInt(stmt, column)

    float readFloat(string column)
        return sqliteReadFloat(stmt, column)

    string readString(string column)
        return sqliteReadString(stmt, column)

    char[] readBytes(string column)
        return sqliteReadBytes(stmt, column)

    bool readNull(string column)
        return sqliteReadNull(stmt, column)

    bool bind(int index, int value)
        return sqliteBind(stmt, index, value)

    bool bind(int index, float value)
        return sqliteBind(stmt, index, value)

    bool bind(int index, string value)
        return sqliteBind(stmt, index, value)
    
    bool bind(int index, char[] value)
        return sqliteBind(stmt, index, value)
    
    bool bind(int index)
        return sqliteBindNull(stmt, index)

    bool next()
        return sqliteNext(stmt)

class JsonNumber
    float value

    void __init__()
    void __init__(float value)
        this.value = value

any jsonNumber(float value)
    return JsonNumber(value)

class JsonString
    string value

    void __init__()
    void __init__(string value)
        this.value = value

any jsonString(string value)
    return JsonString(value)

class JsonBool
    bool value

    void __init__()
    void __init__(bool value)
        this.value = value

any jsonBool(bool value)
    return JsonBool(value)

class JsonArray
    any[] value

    void __init__()
    void __init__(any[] value)
        this.value = value

    any __get__(int index)
        return this.value[index]
    
    void push(bool value)
        this.value.push(JsonBool(value))

    void push(float value)
        this.value.push(JsonNumber(value))

    void push(string value)
        this.value.push(JsonString(value))
    
    void push(JsonArray value)
        this.value.push(value)
    
    void push(JsonObject value)
        this.value.push(value)

    int length()
        return this.value.length

any jsonArray(any[] value)
    return JsonArray(value)

class JsonObject
    Map<string, any> value

    void __init__()
        this.value = Map<string, any>()

    void __init__(Map<string, any> value)
        this.value = value

    void __set__(string key, bool val)
        this.value[key] = JsonBool(val)
        
    void __set__(string key, float val)
        this.value[key] = JsonNumber(val)

    void __set__(string key, string val)
        this.value[key] = JsonString(val)

    void __set__(string key, JsonArray val)
        this.value[key] = val
    
    void __set__(string key, JsonObject val)
        this.value[key] = val

    any __get__(string key)
        return this.value[key]

any jsonObject(Map<string, any> value)
    return JsonObject(value)

void jsonEncodeString(char[] buffer, string value)
    buffer.push('"')

    for char c in value
        if c == '"'
            buffer.push('\\')
            buffer.push('"')
        else if c == '\\'
            buffer.push('\\')
            buffer.push('\\')
        else if c == '\n'
            buffer.push('\\')
            buffer.push('n')
        else if c == '\r'
            buffer.push('\\')
            buffer.push('r')
        else if c == '\t'
            buffer.push('\\')
            buffer.push('t')
        else
            buffer.push(c)

    buffer.push('"')

string jsonEncode(any value)
    char[] buffer
    jsonEncode(buffer, value)

    return buffer.toString()

void jsonEncode(char[] buffer, any value)
    if value is JsonBool
        JsonBool b = (JsonBool)value

        if b.value
            buffer.pushString("true")
        else
            buffer.pushString("false")

    else if value is JsonNumber
        JsonNumber n = (JsonNumber)value
        buffer.pushString((string)n.value)

    else if value is JsonString
        JsonString s = (JsonString)value
        jsonEncodeString(buffer, s.value)

    else if value is JsonArray
        JsonArray a = (JsonArray)value
        buffer.push('[')

        for any value in a.value
            if it > 0
                buffer.push(',')
            
            jsonEncode(buffer, value)

        buffer.push(']')
    
    else if value is JsonObject
        JsonObject o = (JsonObject)value
        buffer.push('{')
        bool first = true

        for bool used in o.value.used
            if not used
                continue

            if not first
                buffer.push(',')

            jsonEncodeString(buffer, o.value.keys[it])
            buffer.push(':')
            jsonEncode(buffer, o.value.values[it])

            first = false

        buffer.push('}')
    else
        buffer.pushString("null")

class Map<K, V>
    K[] keys
    V[] values
    bool[] used
    int bucketCount
    int size

    void __init__()
        bucketCount = 32
        size = 0

        keys.reserve(bucketCount)
        values.reserve(bucketCount)
        used.reserve(bucketCount)
    
    void __set__(K key, V value)
        insertAndResize(key, value)

    V __get__(K key)
        return get(key)

    int hash(K key)
        int h = key.hash() % bucketCount
        if h < 0
            h *= -1
        return h

    void insert(K key, V value)
        int index = hash(key)

        while used[index]
            if keys[index] == key
                values[index] = value
                return
            index = (index + 1) % bucketCount

        keys[index] = key
        values[index] = value
        used[index] = true
        size += 1

    void insertAndResize(K key, V value)
        void resize()
            K[] oldKeys = keys
            V[] oldValues = values
            bool[] oldUsed = used
            int oldCount = bucketCount

            bucketCount = bucketCount * 2
            size = 0
            keys.reserve(bucketCount)
            values.reserve(bucketCount)
            used.reserve(bucketCount)

            for int i = 0; i < oldCount; i += 1
                if oldUsed[i]
                    insert(oldKeys[i], oldValues[i])

        insert(key, value)

        float threshold = 0.75
        if size > bucketCount * threshold
            resize()

    bool contains(K key)
        int index = hash(key)
        int start = index

        while used[index]
            if keys[index] == key
                return true
            index = (index + 1) % bucketCount
            if index == start
                return false

        return false

    V get(K key)
        int index = hash(key)
        int start = index

        while used[index]
            if keys[index] == key
                return values[index]
            index = (index + 1) % bucketCount
            if index == start
                break

        return V()

    void remove(K key)
        int index = hash(key)
        int start = index

        while used[index]
            if keys[index] == key
                used[index] = false
                size -= 1

                int next = (index + 1) % bucketCount
                while used[next]
                    K rehashKey = keys[next]
                    V rehashValue = values[next]
                    used[next] = false
                    size -= 1
                    insert(rehashKey, rehashValue)
                    next = (next + 1) % bucketCount

                return

            index = (index + 1) % bucketCount
            if index == start
                return
"#;

static mut CONTEXT: *mut Context = ptr::null_mut();

fn read_script(path: &String) -> (String, Vec<String>, Rc<Vec<(i32, i32)>>) {
    fn dedent(
        input: &str,
        mapping: &mut Vec<(i32, i32)>,
        mut line: i32,
        mut column: i32,
    ) -> String {
        let lines: Vec<&str> = input.lines().collect();

        let min_indent = lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(0);

        let mut result = String::new();

        for text in lines {
            if text.trim().is_empty() {
                line += 1;
                column = 1;

                continue;
            }

            let start = cmp::min(min_indent, text.len() - text.trim_start().len());
            result.push_str(&text[start..]);
            result.push('\n');

            mapping.push((line, (column - 1) + start as i32));

            line += 1;
            column = 1;
        }

        result
    }

    let input = fs::read_to_string(path).unwrap();
    let mut output = String::new();

    let mut code = false;
    let mut text = Vec::<String>::new();
    let mut mapping = Vec::<(i32, i32)>::new();
    let mut line = 1;
    let mut column = 1;

    let mut start = 0;
    let mut start_line = 1;
    let mut start_column = 1;

    for i in 0..input.len() {
        if input.as_bytes()[i] == b'\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }

        if code {
            if input.as_bytes()[i] == b'?' && i + 1 < input.len() && input.as_bytes()[i + 1] == b'>'
            {
                output += &dedent(&input[start..i], &mut mapping, start_line, start_column);

                start_column = column + 1;
                start_line = line;
                start = i + 2;
                code = false;
            }
        } else {
            if input.as_bytes()[i] == b'<' && i + 1 < input.len() && input.as_bytes()[i + 1] == b'?'
            {
                if start < i {
                    output += "printInternal(";
                    output += &text.len().to_string();
                    output += ")\n";

                    mapping.push((start_line, start_column - 1));
                    text.push(input[start..i].to_owned());
                }

                start_column = column + 1;
                start_line = line;
                start = i + 2;
                code = true;
            }
        }
    }

    if code {
        output += &dedent(&input[start..], &mut mapping, start_line, start_column);
    } else {
        if start < input.len() {
            output += "printInternal(";
            output += &text.len().to_string();
            output += ")\n";

            mapping.push((start_line, start_column - 1));
            text.push(input[start..].to_owned());
        }
    }

    assert_eq!(output.lines().count(), mapping.len());

    mapping.push((line, column - 1));

    (output, text, Rc::new(mapping))
}

fn run_script(req: &mut Request, context: &mut Context, script: &Script) {
    let instant = Instant::now();
    context.text = script.text.clone();
    context.mapping = script.mapping.clone();
    context.functions = script.functions;
    context.environs = req.params();
    context.headers.clear();
    context.output.clear();
    context.input.clear();
    req.stdin().read_to_string(&mut context.input).unwrap();

    unsafe { cyth_run(script.vm) };

    if !context.headers.contains("Content-Type:") {
        context
            .headers
            .push_str("Content-Type: text/html; charset=UTF-8\n");
    }

    write!(
        &mut req.stdout(),
        "Interval: {:?}\nContent-Length: {}\n{}\n{}",
        instant.elapsed(),
        context.output.len(),
        context.headers,
        context.output
    )
    .unwrap_or(());

    context.statements.clear();
    context.connections.clear();
}

fn compile_script(vm: *const c_void) -> c_int {
    unsafe {
        unsafe extern "C" fn print(input: *const CyString) {
            let context = unsafe { &mut *CONTEXT };
            context.output.push_str(cyth_string_to_str(input));
        }
        cyth_load_function(vm, c"void print(string n)".as_ptr(), print as *const c_void);

        unsafe extern "C" fn println(input: *const CyString) {
            let context = unsafe { &mut *CONTEXT };
            context.output.push_str(cyth_string_to_str(input));
            context.output.push('\n');
        }
        cyth_load_function(
            vm,
            c"void println(string n)".as_ptr(),
            println as *const c_void,
        );

        unsafe extern "C" fn print_internal(n: i32) {
            let context = unsafe { &mut *CONTEXT };

            if let Some(text) = context.text.get(n as usize) {
                context.output.write_str(text).unwrap();
            }
        }
        cyth_load_function(
            vm,
            c"void printInternal(int n)".as_ptr(),
            print_internal as *const c_void,
        );

        unsafe extern "C" fn parse_int(input: *const CyString, radix: c_int) -> c_int {
            let input = cyth_string_to_str(input);

            c_int::from_str_radix(input, radix as u32).unwrap_or_default()
        }
        cyth_load_function(
            vm,
            c"int parseInt(string n, int m)".as_ptr(),
            parse_int as *const c_void,
        );

        unsafe extern "C" fn parse_int2(input: *const CyString) -> c_int {
            let input = cyth_string_to_str(input);

            c_int::from_str_radix(input, 10).unwrap_or_default()
        }
        cyth_load_function(
            vm,
            c"int parseInt(string n)".as_ptr(),
            parse_int2 as *const c_void,
        );

        unsafe extern "C" fn parse_float(input: *const CyString) -> c_float {
            let input = cyth_string_to_str(input);

            input.parse::<f32>().unwrap_or_default()
        }
        cyth_load_function(
            vm,
            c"float parseFloat(string n)".as_ptr(),
            parse_float as *const c_void,
        );

        unsafe extern "C" fn url_encode(input: *const CyString) -> *const CyString {
            let input = cyth_string_to_str(input);
            let output = percent_encoding::utf8_percent_encode(input, NON_ALPHANUMERIC).to_string();

            cyth_new_string(&output)
        }
        cyth_load_function(
            vm,
            c"string urlEncode(string n)".as_ptr(),
            url_encode as *const c_void,
        );

        unsafe extern "C" fn url_decode(input: *const CyString) -> *const CyString {
            let input = cyth_string_to_str(input);
            let output = percent_encoding::percent_decode_str(input)
                .decode_utf8()
                .unwrap_or("".into())
                .into_owned()
                .replace("+", " ");

            cyth_new_string(&output)
        }
        cyth_load_function(
            vm,
            c"string urlDecode(string n)".as_ptr(),
            url_decode as *const c_void,
        );

        unsafe extern "C" fn markdown(input: *const CyString) -> *const CyString {
            let input = cyth_string_to_str(input);
            let output = markdown::to_html_with_options(
                input,
                &Options {
                    compile: CompileOptions {
                        allow_dangerous_html: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap_or("".to_owned());

            cyth_new_string(&output)
        }
        cyth_load_function(
            vm,
            c"string markdown(string n)".as_ptr(),
            markdown as *const c_void,
        );

        unsafe extern "C" fn hash(input: *const CyString) -> *const CyString {
            let input = cyth_string_to_str(input);
            let output = bcrypt::hash(input, DEFAULT_COST).unwrap();
            cyth_new_string(&output)
        }
        cyth_load_function(vm, c"string hash(string n)".as_ptr(), hash as *const c_void);

        unsafe extern "C" fn verify(password: *const CyString, hash: *const CyString) -> c_int {
            let password = cyth_string_to_str(password);
            let hash = cyth_string_to_str(hash);
            let output = bcrypt::verify(password, hash).unwrap();

            output.into()
        }
        cyth_load_function(
            vm,
            c"bool verify(string n, string m)".as_ptr(),
            verify as *const c_void,
        );

        unsafe extern "C" fn body() -> *const CyString {
            let context = unsafe { &mut *CONTEXT };

            cyth_new_string(&context.input)
        }
        cyth_load_function(vm, c"string body()".as_ptr(), body as *const c_void);

        unsafe extern "C" fn query() -> *const CyString {
            let context = unsafe { &mut *CONTEXT };
            let default = String::new();
            let query = context.environs.get("QUERY_STRING").unwrap_or(&default);

            cyth_new_string(query)
        }
        cyth_load_function(vm, c"string query()".as_ptr(), query as *const c_void);

        unsafe extern "C" fn header(input: *const CyString) {
            let context = unsafe { &mut *CONTEXT };
            let input = cyth_string_to_str(input);

            context.headers.push_str(input);
            context.headers.push('\n');
        }
        cyth_load_function(
            vm,
            c"void header(string n)".as_ptr(),
            header as *const c_void,
        );

        unsafe extern "C" fn cookie(name: *const CyString) -> *const CyString {
            let context = unsafe { &mut *CONTEXT };
            let name = cyth_string_to_str(name);
            let empty_string = "".to_owned();
            let cookie = context.environs.get("HTTP_COOKIE").unwrap_or(&empty_string);

            if let Some(mut start) = cookie.find(&(name.to_owned() + "=")) {
                start += name.len() + 1;

                let mut end = start;
                while end < cookie.len() {
                    if cookie.as_bytes()[end] == b';' {
                        break;
                    }

                    end += 1;
                }

                let result = &cookie[start..end].trim().to_owned();
                cyth_new_string(result)
            } else {
                cyth_new_string(&empty_string)
            }
        }
        cyth_load_function(
            vm,
            c"string cookie(string n)".as_ptr(),
            cookie as *const c_void,
        );

        unsafe extern "C" fn uuid() -> *const CyString {
            let uuid = Uuid::new_v4();
            cyth_new_string(&uuid.as_hyphenated().to_string())
        }
        cyth_load_function(vm, c"string uuid()".as_ptr(), uuid as *const c_void);

        unsafe extern "C" fn get_environ(key: *const CyString) -> *const CyString {
            let context = unsafe { &mut *CONTEXT };
            let key = cyth_string_to_str(key);

            let empty_string = "".to_owned();
            let environ = context.environs.get(key).unwrap_or(&empty_string);

            cyth_new_string(environ)
        }
        cyth_load_function(
            vm,
            c"string getEnviron(string n)".as_ptr(),
            get_environ as *const c_void,
        );

        unsafe extern "C" fn get_environs() -> *const CyArray<*mut CyString> {
            let context = unsafe { &mut *CONTEXT };

            cyth_new_array(
                context
                    .environs
                    .keys()
                    .map(|key| cyth_new_string(key.as_str())),
            )
        }
        cyth_load_function(
            vm,
            c"string[] getEnvirons()".as_ptr(),
            get_environs as *const c_void,
        );

        unsafe extern "C" fn date(epoch: c_int, format: *const CyString) -> *const CyString {
            let format = cyth_string_to_str(format);
            let datetime = DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(epoch as u64));
            let result = datetime.format(format).to_string();

            cyth_new_string(&result)
        }
        cyth_load_function(
            vm,
            c"string date(int n, string m)".as_ptr(),
            date as *const c_void,
        );

        unsafe extern "C" fn now() -> c_int {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i32
        }
        cyth_load_function(vm, c"int now()".as_ptr(), now as *const c_void);

        unsafe extern "C" fn fetch(uri: *const CyString) -> *const CyString {
            let uri = cyth_string_to_str(uri);

            if let Ok(mut request) = ureq::get(uri).call() {
                let body = request.body_mut().read_to_string().unwrap_or_default();

                cyth_new_string(&body)
            } else {
                cyth_new_string("")
            }
        }
        cyth_load_function(
            vm,
            c"string fetch(string n)".as_ptr(),
            fetch as *const c_void,
        );

        unsafe extern "C" fn json_decode(json: *const CyString) -> *const c_void {
            let json = cyth_string_to_str(json);
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
                return cyth_new_json(&value);
            }

            null()
        }
        cyth_load_function(
            vm,
            c"any jsonDecode(string n)".as_ptr(),
            json_decode as *const c_void,
        );

        unsafe extern "C" fn sqlite_open(path: *const CyString) -> c_int {
            let context = unsafe { &mut *CONTEXT };
            let path = cyth_string_to_str(path);
            let connection = Connection::open_thread_safe(path);

            match connection {
                Ok(connection) => {
                    context.connections.push(connection);
                    context.connections.len() as c_int
                }
                Err(error) => {
                    println!("{:?}", error.message);

                    0
                }
            }
        }
        cyth_load_function(
            vm,
            c"int sqliteOpen(string n)".as_ptr(),
            sqlite_open as *const c_void,
        );

        unsafe extern "C" fn sqlite_execute(id: c_int, query: *const CyString) -> c_int {
            let context = unsafe { &mut *CONTEXT };
            let query = cyth_string_to_str(query);
            let Some(connection) = context.connections.get_mut((id - 1) as usize) else {
                return 0;
            };

            connection.execute(query).is_ok() as c_int
        }
        cyth_load_function(
            vm,
            c"bool sqliteExecute(int n, string m)".as_ptr(),
            sqlite_execute as *const c_void,
        );

        unsafe extern "C" fn sqlite_prepare(id: c_int, query: *const CyString) -> c_int {
            let context = unsafe { &mut *CONTEXT };

            let query = cyth_string_to_str(query);
            let Some(connection) = context.connections.get_mut((id - 1) as usize) else {
                println!(
                    "Failed to get connection: {} {} {}",
                    query,
                    id,
                    context.connections.len()
                );
                return 0;
            };

            match connection.prepare(query) {
                Ok(statement) => {
                    context.statements.push(statement);
                    context.statements.len() as c_int
                }
                Err(error) => {
                    println!("{:?}", error.message);
                    0
                }
            }
        }
        cyth_load_function(
            vm,
            c"int sqlitePrepare(int n, string m)".as_ptr(),
            sqlite_prepare as *const c_void,
        );

        unsafe extern "C" fn sqlite_bind_int(id: c_int, index: c_int, value: c_int) -> c_int {
            let context = unsafe { &mut *CONTEXT };
            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return 0;
            };

            statement.bind((index as usize, value as i64)).is_ok() as c_int
        }
        cyth_load_function(
            vm,
            c"bool sqliteBind(int n, int m, int q)".as_ptr(),
            sqlite_bind_int as *const c_void,
        );

        unsafe extern "C" fn sqlite_bind_float(id: c_int, index: c_int, value: c_float) -> c_int {
            let context = unsafe { &mut *CONTEXT };
            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return 0;
            };

            statement.bind((index as usize, value as f64)).is_ok() as c_int
        }
        cyth_load_function(
            vm,
            c"bool sqliteBind(int n, int m, float q)".as_ptr(),
            sqlite_bind_float as *const c_void,
        );

        unsafe extern "C" fn sqlite_bind_char(
            id: c_int,
            index: c_int,
            value: *const CyArray<u8>,
        ) -> c_int {
            let context = unsafe { &mut *CONTEXT };
            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return 0;
            };

            let slice = cyth_char_array_to_slice(value);
            statement.bind((index as usize, slice)).is_ok() as c_int
        }
        cyth_load_function(
            vm,
            c"bool sqliteBind(int n, int m, char[] q)".as_ptr(),
            sqlite_bind_char as *const c_void,
        );

        unsafe extern "C" fn sqlite_bind_string(
            id: c_int,
            index: c_int,
            value: *const CyString,
        ) -> c_int {
            let context = unsafe { &mut *CONTEXT };
            let value = cyth_string_to_str(value);
            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return 0;
            };

            statement.bind((index as usize, value)).is_ok() as c_int
        }
        cyth_load_function(
            vm,
            c"bool sqliteBind(int n, int m, string q)".as_ptr(),
            sqlite_bind_string as *const c_void,
        );

        unsafe extern "C" fn sqlite_bind_null(id: c_int, index: c_int) -> c_int {
            let context = unsafe { &mut *CONTEXT };
            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return 0;
            };

            statement
                .bind((index as usize, sqlite::Value::Null))
                .is_ok() as c_int
        }
        cyth_load_function(
            vm,
            c"bool sqliteBindNull(int n, int m)".as_ptr(),
            sqlite_bind_null as *const c_void,
        );

        unsafe extern "C" fn sqlite_next(id: c_int) -> c_int {
            let context = unsafe { &mut *CONTEXT };
            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return 0;
            };

            let state = statement.next();

            match state {
                Ok(state) => {
                    if state == State::Row {
                        1
                    } else {
                        0
                    }
                }
                Err(_) => 0,
            }
        }
        cyth_load_function(
            vm,
            c"bool sqliteNext(int n)".as_ptr(),
            sqlite_next as *const c_void,
        );

        unsafe extern "C" fn sqlite_read_int(id: c_int, value: *const CyString) -> c_int {
            let context = unsafe { &mut *CONTEXT };
            let value = cyth_string_to_str(value);
            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return 0;
            };

            statement.read::<i64, &str>(value).unwrap_or_default() as c_int
        }
        cyth_load_function(
            vm,
            c"int sqliteReadInt(int n, string m)".as_ptr(),
            sqlite_read_int as *const c_void,
        );

        unsafe extern "C" fn sqlite_read_float(id: c_int, value: *const CyString) -> c_float {
            let context = unsafe { &mut *CONTEXT };
            let value = cyth_string_to_str(value);
            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return 0.0;
            };

            statement.read::<f64, &str>(value).unwrap_or_default() as c_float
        }
        cyth_load_function(
            vm,
            c"float sqliteReadFloat(int n, string m)".as_ptr(),
            sqlite_read_float as *const c_void,
        );

        unsafe extern "C" fn sqlite_read_char(
            id: c_int,
            value: *const CyString,
        ) -> *const CyArray<u8> {
            let context = unsafe { &mut *CONTEXT };
            let value = cyth_string_to_str(value);

            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return cyth_new_array([].iter().copied());
            };

            let bytes = statement.read::<Vec<u8>, &str>(value).unwrap_or_default();
            cyth_new_array(bytes.iter().copied())
        }
        cyth_load_function(
            vm,
            c"char[] sqliteReadBytes(int n, string m)".as_ptr(),
            sqlite_read_char as *const c_void,
        );

        unsafe extern "C" fn sqlite_read_string(
            id: c_int,
            value: *const CyString,
        ) -> *const CyString {
            let context = unsafe { &mut *CONTEXT };
            let value = cyth_string_to_str(value);
            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return cyth_new_string("");
            };

            cyth_new_string(&statement.read::<String, &str>(value).unwrap_or_default())
        }
        cyth_load_function(
            vm,
            c"string sqliteReadString(int n, string m)".as_ptr(),
            sqlite_read_string as *const c_void,
        );

        unsafe extern "C" fn sqlite_read_null(id: c_int, value: *const CyString) -> c_int {
            let context = unsafe { &mut *CONTEXT };
            let value = cyth_string_to_str(value);
            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return 0;
            };

            let result: Value = statement.read(value).unwrap_or_default();

            (result == Value::Null) as c_int
        }
        cyth_load_function(
            vm,
            c"bool sqliteReadNull(int n, string m)".as_ptr(),
            sqlite_read_null as *const c_void,
        );

        cyth_compile(vm)
    }
}

fn request(mut req: Request, context: &mut Context, scripts: &mut HashMap<String, Script>) {
    let Some(path) = req.param("SCRIPT_FILENAME") else {
        write!(
            &mut req.stdout(),
            "{}{}{}",
            "Status: 500 Internal Server Error\n",
            "Content-Type: text/plain\n\n",
            "Missing 'SCRIPT_FILENAME' environment variable"
        )
        .unwrap_or(());
        return;
    };

    let Ok(metadata) = fs::metadata(&path) else {
        write!(
            &mut req.stdout(),
            "{}{}",
            "Status: 404 Not Found\n",
            "Content-Type: text/plain\n\n"
        )
        .unwrap_or(());
        return;
    };

    let script = scripts.get_mut(&path);
    if script.is_none()
        || script
            .as_ref()
            .unwrap()
            .modified
            .ne(&metadata.modified().unwrap())
    {
        unsafe {
            let (source, text, mapping) = read_script(&path);
            context.path = path;
            context.mapping = mapping.clone();
            context.output.clear();

            let builtins = CString::new(BUILTINS).unwrap();
            let builtins_filename = c"<builtin>";
            let source = CString::new(source).unwrap();
            let source_filename = CString::new(context.path.clone()).unwrap();

            let vm = cyth_init();
            cyth_set_error_callback(vm, error_callback as *const c_void);
            cyth_set_panic_callback(vm, panic_callback as *const c_void);
            cyth_load_string(vm, builtins_filename.as_ptr(), builtins.as_ptr());

            let load_result = cyth_load_string(vm, source_filename.as_ptr(), source.as_ptr());
            let compilation_result = compile_script(vm);

            if load_result == 0 || compilation_result == 0 {
                cyth_destroy(vm);

                write!(
                    &mut req.stdout(),
                    "{}{}{}",
                    "Status: 500 Internal Server Error\n",
                    "Content-Type: text/plain\n\n",
                    context.output
                )
                .unwrap_or(());
                return;
            }

            if let Some(script) = script {
                cyth_destroy(script.vm);
            }

            let script = Script {
                vm,
                mapping,
                modified: metadata.modified().unwrap(),
                text: text.into(),
                functions: Functions {
                    json_number: std::mem::transmute(cyth_get_function(
                        vm,
                        c"jsonNumber.any(float)".as_ptr(),
                    )),
                    json_bool: std::mem::transmute(cyth_get_function(
                        vm,
                        c"jsonBool.any(bool)".as_ptr(),
                    )),
                    json_string: std::mem::transmute(cyth_get_function(
                        vm,
                        c"jsonString.any(string)".as_ptr(),
                    )),
                    json_array: std::mem::transmute(cyth_get_function(
                        vm,
                        c"jsonArray.any(any[])".as_ptr(),
                    )),
                    json_object: std::mem::transmute(cyth_get_function(
                        vm,
                        c"jsonObject.any(Map<string, any>)".as_ptr(),
                    )),
                    map_init: std::mem::transmute(cyth_get_function(
                        vm,
                        c"Map<string, any>".as_ptr(),
                    )),
                    map_set: std::mem::transmute(cyth_get_function(
                        vm,
                        c"Map<string, any>.__set__.void(string, any)".as_ptr(),
                    )),
                },
            };

            run_script(&mut req, context, &script);
            scripts.insert(context.path.clone(), script);
        }
    } else {
        unsafe {
            let context = &mut *CONTEXT;
            let script = script.unwrap();
            run_script(&mut req, context, script);
        }
    }
}

fn main() -> ExitCode {
    if env::args().count() < 1 {
        println!("usage: cyth-cgi [listen address]");
        return ExitCode::FAILURE;
    }

    let context = Box::leak(Box::new(Context::default()));
    let mut scripts = HashMap::<String, Script>::new();

    unsafe {
        CONTEXT = context as *mut Context;
    };

    if env::args().count() > 1 {
        let listener = TcpListener::bind(args().nth(1).unwrap()).unwrap();
        fastcgi::run_tcp(move |req| request(req, context, &mut scripts), &listener);
    } else {
        #[cfg(unix)]
        fastcgi::run(move |req| request(req, context, &mut scripts));

        #[cfg(windows)]
        {
            use std::os::windows::io::{FromRawSocket, RawSocket};
            use windows::Win32::Networking::WinSock::{WSADATA, WSAStartup};
            use windows::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE};

            unsafe {
                let mut data = WSADATA::default();
                let result = WSAStartup(0x0202, &mut data);
                if result != 0 {
                    panic!("WSAStartup failed: {}", result);
                }

                let handle = GetStdHandle(STD_INPUT_HANDLE);
                let socket = handle.unwrap().0 as RawSocket;
                let listener =
                    TcpListener::from_raw_socket(socket as std::os::windows::io::RawSocket);

                fastcgi::run_tcp(move |req| request(req, context, &mut scripts), &listener);
            }
        }
    }

    ExitCode::SUCCESS
}
