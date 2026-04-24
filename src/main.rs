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
    ptr,
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
    vm: *const c_void,
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

fn cyth_new_array<T: Copy>(list: Vec<T>) -> *mut CyArray<T> {
    unsafe {
        let data_size = list.len();
        let data_atomic = !std::any::type_name::<T>().starts_with("*") as i32;
        let data_layout = Layout::array::<T>(data_size).unwrap();
        let data_ptr = cyth_alloc(data_atomic, data_layout.size()) as *mut T;
        if data_ptr.is_null() {
            std::alloc::handle_alloc_error(data_layout);
        }

        for (index, item) in list.iter().enumerate() {
            ptr::write(data_ptr.add(index), *item);
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
        context.output.push_str(&format!(
            "{}:{}:{}-{}:{}: {}\n",
            filename,
            context.mapping[(start_line - 1) as usize].0,
            context.mapping[(start_line - 1) as usize].1 + start_column,
            context.mapping[(end_line - 1) as usize].0,
            context.mapping[(end_line - 1) as usize].1 + end_column,
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
        context
            .headers
            .push_str("Status: 500 Internal Server Error\n");
        context.headers.push_str("Content-Type: text/plain\n");

        context.output.clear();
        context.output.push_str(&format!("{}\n", unsafe {
            CStr::from_ptr(function).to_str().unwrap()
        }));
    } else {
        context.output.push_str(&format!(
            "  at {}:{}:{}\n",
            unsafe { CStr::from_ptr(function).to_str().unwrap() },
            context.mapping[(line - 1) as usize].0,
            context.mapping[(line - 1) as usize].1 + column,
        ));
    }
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
}

const BUILTINS: &str = "
Map<string, string> parseQuery(string query)
    Map<string, string> result = Map<string, string>()

    string[] pairs = query.split(\"&\")
    for string pair in pairs
        string[] parts = pair.split(\"=\")

        if parts.length == 2
            result.insert(parts[0], urlDecode(parts[1]))

    return result

int parseInt(string n, int base)
    n = n.trim()
    if not n
        return 0

    int index = 0
    bool negative = false

    if n[0] == '+'
        index += 1
    else if n[0] == '-'
        negative = true
        index += 1

    int value = 0
    while index < n.length
        char c = n[index]
        int digit

        if c >= '0' and c <= '9'
            digit = c - '0'
        else if c >= 'A' and c <= 'Z'
            digit = c - 'A' + 10
        else if c >= 'a' and c <= 'z'
            digit = c - 'a' + 10
        else
            break

        if digit >= base
            break

        value = value * base + digit
        index += 1

    if negative
        value = -value

    return value

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

class Entry<K, V>
    K key
    V value
    Entry<K, V> next

    void __init__(K key, V value, Entry<K, V> next)
        this.key = key
        this.value = value
        this.next = next

class Map<K, V>
    Entry<K, V>[] buckets
    int bucketCount
    int size

    void __init__()
        bucketCount = 64
        size = 0

        for int i = 0; i < bucketCount; i += 1
            buckets.push(null)

    void __set__(K key, V value)
        insert(key, value)

    V __get__(K key)
        return get(key)

    int hash(K key)
        int hash = key.hash() % buckets.length

        if hash < 0
            hash = hash * -1
        
        return hash

    void insert(K key, V value)
        void resize()
            Entry<K, V>[] oldBuckets = buckets
            bucketCount = bucketCount * 2
            size = 0

            for int i = 0; i < bucketCount; i += 1
                buckets.push(null)

            for int i = 0; i < oldBuckets.length; i += 1
                Entry<K, V> current = oldBuckets[i]
                while current != null
                    Entry<K, V> nextEntry = current.next
                    insert(current.key, current.value)
                    current = nextEntry

        int index = hash(key)
        Entry<K, V> head = buckets[index]
        Entry<K, V> current = head

        while current != null
            if current.key == key
                current.value = value
                return
            
            current = current.next
    
        Entry<K, V> newEntry = Entry<K, V>(key, value, head)
        buckets[index] = newEntry
        size = size + 1
    
        float threshold = 0.75
        if size > bucketCount * threshold
            resize()

    bool contains(K key)
        int index = hash(key)
        Entry<K, V> current = buckets[index]

        while current != null
            if current.key == key
                return true

            current = current.next

        return false

    V get(K key)
        int index = hash(key)
        Entry<K, V> current = buckets[index]

        while current != null
            if current.key == key
                return current.value

            current = current.next
    
        current = null
        return current.value

    void remove(K key)
        int index = hash(key)
        Entry<K, V> current = buckets[index]
        Entry<K, V> prev = null

        while current != null
            if current.key == key
                if prev == null
                    buckets[index] = current.next
                else
                    prev.next = current.next
                
                size = size - 1
                return
        
            prev = current
            current = current.next
";

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
        cyth_load_function(
            vm,
            CString::new("void print(string n)").unwrap().as_ptr(),
            print as *const c_void,
        );

        unsafe extern "C" fn println(input: *const CyString) {
            let context = unsafe { &mut *CONTEXT };
            context.output.push_str(cyth_string_to_str(input));
            context.output.push('\n');
        }
        cyth_load_function(
            vm,
            CString::new("void println(string n)").unwrap().as_ptr(),
            println as *const c_void,
        );

        unsafe extern "C" fn print_internal(n: i32) {
            let context = unsafe { &mut *CONTEXT };

            if let Some(text) = context.text.get(n as usize) {
                context.output.write_str(&text).unwrap();
            }
        }
        cyth_load_function(
            vm,
            CString::new("void printInternal(int n)").unwrap().as_ptr(),
            print_internal as *const c_void,
        );

        unsafe extern "C" fn url_encode(input: *const CyString) -> *const CyString {
            let input = cyth_string_to_str(input);
            let output = percent_encoding::utf8_percent_encode(input, NON_ALPHANUMERIC).to_string();

            cyth_new_string(&output)
        }
        cyth_load_function(
            vm,
            CString::new("string urlEncode(string n)").unwrap().as_ptr(),
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
            CString::new("string urlDecode(string n)").unwrap().as_ptr(),
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
            CString::new("string markdown(string n)").unwrap().as_ptr(),
            markdown as *const c_void,
        );

        unsafe extern "C" fn hash(input: *const CyString) -> *const CyString {
            let input = cyth_string_to_str(input);
            let output = bcrypt::hash(input, DEFAULT_COST).unwrap();
            cyth_new_string(&output)
        }
        cyth_load_function(
            vm,
            CString::new("string hash(string n)").unwrap().as_ptr(),
            hash as *const c_void,
        );

        unsafe extern "C" fn verify(password: *const CyString, hash: *const CyString) -> c_int {
            let password = cyth_string_to_str(password);
            let hash = cyth_string_to_str(hash);
            let output = bcrypt::verify(password, hash).unwrap();

            output.into()
        }
        cyth_load_function(
            vm,
            CString::new("bool verify(string n, string m)")
                .unwrap()
                .as_ptr(),
            verify as *const c_void,
        );

        unsafe extern "C" fn body() -> *const CyString {
            let context = unsafe { &mut *CONTEXT };

            cyth_new_string(&context.input)
        }
        cyth_load_function(
            vm,
            CString::new("string body()").unwrap().as_ptr(),
            body as *const c_void,
        );

        unsafe extern "C" fn query() -> *const CyString {
            let context = unsafe { &mut *CONTEXT };
            let default = String::new();
            let query = context.environs.get("QUERY_STRING").unwrap_or(&default);

            cyth_new_string(query)
        }
        cyth_load_function(
            vm,
            CString::new("string query()").unwrap().as_ptr(),
            query as *const c_void,
        );

        unsafe extern "C" fn header(input: *const CyString) {
            let context = unsafe { &mut *CONTEXT };
            let input = cyth_string_to_str(input);

            context.headers.push_str(input);
            context.headers.push('\n');
        }
        cyth_load_function(
            vm,
            CString::new("void header(string n)").unwrap().as_ptr(),
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
            CString::new("string cookie(string n)").unwrap().as_ptr(),
            cookie as *const c_void,
        );

        unsafe extern "C" fn uuid() -> *const CyString {
            let uuid = Uuid::new_v4();
            cyth_new_string(&uuid.as_hyphenated().to_string())
        }
        cyth_load_function(
            vm,
            CString::new("string uuid()").unwrap().as_ptr(),
            uuid as *const c_void,
        );

        unsafe extern "C" fn get_environ(key: *const CyString) -> *const CyString {
            let context = unsafe { &mut *CONTEXT };
            let key = cyth_string_to_str(key);

            let empty_string = "".to_owned();
            let environ = context.environs.get(key).unwrap_or(&empty_string);

            cyth_new_string(environ)
        }
        cyth_load_function(
            vm,
            CString::new("string getEnviron(string n)")
                .unwrap()
                .as_ptr(),
            get_environ as *const c_void,
        );

        unsafe extern "C" fn get_environs() -> *mut CyArray<*mut CyString> {
            let context = unsafe { &mut *CONTEXT };

            cyth_new_array(
                context
                    .environs
                    .keys()
                    .map(|key| cyth_new_string(key.as_str()))
                    .collect(),
            )
        }
        cyth_load_function(
            vm,
            CString::new("string[] getEnvirons()").unwrap().as_ptr(),
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
            CString::new("string date(int n, string m)")
                .unwrap()
                .as_ptr(),
            date as *const c_void,
        );

        unsafe extern "C" fn now() -> c_int {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i32
        }
        cyth_load_function(
            vm,
            CString::new("int now()").unwrap().as_ptr(),
            now as *const c_void,
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
            CString::new("int sqliteOpen(string n)").unwrap().as_ptr(),
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
            CString::new("bool sqliteExecute(int n, string m)")
                .unwrap()
                .as_ptr(),
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
            CString::new("int sqlitePrepare(int n, string m)")
                .unwrap()
                .as_ptr(),
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
            CString::new("bool sqliteBind(int n, int m, int q)")
                .unwrap()
                .as_ptr(),
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
            CString::new("bool sqliteBind(int n, int m, float q)")
                .unwrap()
                .as_ptr(),
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
            CString::new("bool sqliteBind(int n, int m, char[] q)")
                .unwrap()
                .as_ptr(),
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
            CString::new("bool sqliteBind(int n, int m, string q)")
                .unwrap()
                .as_ptr(),
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
            CString::new("bool sqliteBindNull(int n, int m)")
                .unwrap()
                .as_ptr(),
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
            CString::new("bool sqliteNext(int n)").unwrap().as_ptr(),
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
            CString::new("int sqliteReadInt(int n, string m)")
                .unwrap()
                .as_ptr(),
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
            CString::new("float sqliteReadFloat(int n, string m)")
                .unwrap()
                .as_ptr(),
            sqlite_read_float as *const c_void,
        );

        unsafe extern "C" fn sqlite_read_char(
            id: c_int,
            value: *const CyString,
        ) -> *const CyArray<u8> {
            let context = unsafe { &mut *CONTEXT };
            let value = cyth_string_to_str(value);

            let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                return cyth_new_array([].to_vec());
            };

            cyth_new_array(statement.read::<Vec<u8>, &str>(value).unwrap_or_default())
        }
        cyth_load_function(
            vm,
            CString::new("char[] sqliteReadBytes(int n, string m)")
                .unwrap()
                .as_ptr(),
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
            CString::new("string sqliteReadString(int n, string m)")
                .unwrap()
                .as_ptr(),
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
            CString::new("bool sqliteReadNull(int n, string m)")
                .unwrap()
                .as_ptr(),
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
            let builtins_filename = CString::new("<builtin>").unwrap();
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
                modified: metadata.modified().unwrap(),
                text: text.into(),
                mapping,
                vm,
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
            println!("error: unix sockets are not supported on this platform");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}
