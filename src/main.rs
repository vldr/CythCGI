extern crate fastcgi;

use std::{
    any::Any,
    cmp,
    collections::HashMap,
    env::{self, args},
    ffi::c_void,
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::fd::AsRawFd,
    panic::{AssertUnwindSafe, catch_unwind},
    process::{Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bcrypt::{DEFAULT_COST, hash, verify};
use chrono::{DateTime, Local};
use markdown::{CompileOptions, Options};
use percent_encoding::NON_ALPHANUMERIC;
use regex::Regex;

use fastcgi::Request;
use sqlite::{Connection, ConnectionThreadSafe, State, Statement, Value};
use uuid::Uuid;

struct Script {
    modified: SystemTime,
    instance: v8::Global<v8::Function>,
    module: v8::Global<v8::WasmModuleObject>,
}

struct Context {
    input: String,
    output: String,
    headers: String,
    environs: HashMap<String, String>,
    backing: v8::SharedRef<v8::BackingStore>,
    externals: Vec<Box<dyn Any>>,
    to_string: v8::Global<v8::Function>,
    from_string: v8::Global<v8::Function>,
}

const IMPORTS: &str = "import \"env\"
    void print(string a)
    void println(string a)

    string urlEncode(string a)
    string urlDecode(string a)
    string markdown(string a)

    string hash(string a)
    bool verify(string a, string b)

    string body()
    string query()

    void header(string a)
    string cookie(string a)
    string uuid()

    string getEnviron(string a)

    string date(int a, string b)
    int now()

    any sqliteOpen(string a)
    bool sqliteExecute(any a, string b)
    any sqlitePrepare(any a, string b)
    bool sqliteBind<T>(any a, int b, T c)
    bool sqliteBindNull(any a, int b)
    bool sqliteNext(any a)
    bool sqliteReadNull(any a, string b)
    T sqliteRead<T>(any a, string b)

string toString(int length)
    char[] buffer
    buffer.reserve(length)

    for int i = 0; i < length; i += 1
        buffer[i] = readChar(i)

    return buffer.toString()

int fromString(string n)
    for char c in n
        writeChar(it, c)

    return n.length

int stringIndexOf(string s, string target)
    if target.length == 0
        return 0

    for int i = 0; i <= s.length - target.length; i += 1
        bool match = true

        for char c in target
            if s[i + it] != c
                match = false
                break

        if match
            return i

    return -1

bool stringContains(string s, string target)
    return stringIndexOf(s, target) != -1

string stringTrim(string s)
    if not s
        return s

    int start = 0
    int end = s.length - 1

    while start < s.length and (s[start] == ' ' or s[start] == '\t' or s[start] == '\n' or s[start] == '\r')
        start += 1

    while end >= start and (s[end] == ' ' or s[end] == '\t' or s[end] == '\n' or s[end] == '\r')
        end -= 1

    char[] result
    for int i = start; i <= end; i += 1
        result.push(s[i])

    return result.toString()

string[] stringSplit(string s, char delim)
    string[] result
    char[] current

    for char c in s
        if c != delim
            current.push(c)
        else
            result.push(current.toString())
            current.clear()

    result.push(current.toString())
    return result

string stringJoin(string[] parts, string delim)
    string[] result
    char[] buf

    for string part in parts
        for char c in part
            buf.push(c)

        if it != parts.length - 1
            for char c in delim
                buf.push(c)

    return buf.toString()

Map<string, string> parseQuery(string query)
    Map<string, string> result = Map<string, string>()

    string[] pairs = stringSplit(query, '&')
    for string pair in pairs
        string[] parts = stringSplit(pair, '=')

        if parts.length == 2
            result.insert(parts[0], urlDecode(parts[1]))

    return result

int parseInt(string n, int base)
    n = stringTrim(n)
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
    any con

    void __init__(string path)
        this.con = sqliteOpen(path)

    Statement prepare(string query)
        any stmt = sqlitePrepare(con, query)
        if stmt
            return Statement(stmt)

        return null

    bool execute(string a)
        return sqliteExecute(con, a)

class Statement
    any stmt

    void __init__(any stmt)
        this.stmt = stmt

    T read<T>(string column)
        return sqliteRead<T>(stmt, column)

    bool readNull(string column)
        return sqliteReadNull(stmt, column)

    bool bind<T>(int index, T value)
        return sqliteBind<T>(stmt, index, value)

    bool bindNull(int index)
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

fn val_to_string(scope: &mut v8::HandleScope, arg: v8::Local<'_, v8::Value>) -> String {
    let context: &Context = scope.get_slot().unwrap();
    let data = context.backing.data().unwrap();

    let from_string = context.from_string.clone();
    let from_string = v8::Local::new(scope, from_string)
        .call(scope, arg, &[arg])
        .unwrap()
        .int32_value(scope)
        .unwrap();

    let length = from_string as usize;

    let ptr = data.as_ptr() as *const u8;
    let slice = unsafe { std::slice::from_raw_parts(ptr, length) };

    unsafe { std::str::from_utf8_unchecked(slice).to_owned() }
}

fn string_to_val<'a>(scope: &mut v8::HandleScope<'a>, arg: &str) -> v8::Local<'a, v8::Value> {
    let context: &Context = scope.get_slot().unwrap();
    let data = context.backing.data().unwrap();

    unsafe {
        let dst_ptr = data.as_ptr() as *mut u8;
        let src_ptr = arg.as_ptr();
        std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, arg.len());
    }

    let to_string = context.to_string.clone();
    let length = v8::Integer::new(scope, arg.len() as i32).into();
    let undefined = v8::undefined(scope).into();
    let to_string: v8::Local<'a, v8::Value> = v8::Local::new(scope, to_string)
        .call(scope, undefined, &[length])
        .unwrap();

    to_string
}

pub fn extern_to_val<'s, T>(scope: &mut v8::HandleScope<'s>, val: T) -> v8::Local<'s, v8::Value>
where
    T: 'static,
{
    let context: &mut Context = scope.get_slot_mut().unwrap();

    let raw: *mut T = Box::into_raw(Box::new(val));
    context.externals.push(unsafe { Box::from_raw(raw) });

    let external = v8::External::new(scope, raw as *mut c_void);

    external.into()
}

pub fn val_to_extern<'s, T>(val: v8::Local<'s, v8::Value>) -> &'s mut T {
    let external = val.cast::<v8::External>();

    unsafe { &mut *(external.value() as *mut T) }
}

fn read_script(path: &String) -> (String, Vec<(i32, i32)>) {
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
    output += IMPORTS;

    let mut code = false;
    let mut mapping = Vec::<(i32, i32)>::new();
    let mut line = 1;
    let mut column = 1;

    for _ in IMPORTS.lines() {
        mapping.push((0, 0));
    }

    let mut start = 0;
    let mut start_line = 1;
    let mut start_column = 1;

    fn hex_from_digit(num: u8) -> char {
        if num < 10 {
            (b'0' + num) as char
        } else {
            (b'A' + num - 10) as char
        }
    }

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
                    let mut count = 0;
                    mapping.push((start_line, start_column - 1));
                    output += "print(\"";

                    for c in &input.as_bytes()[start..i] {
                        if count == 10000 {
                            mapping.push((start_line, start_column - 1));

                            output += "\")\n";
                            output += "print(\"";

                            count = 0;
                        }

                        output += "\\x";
                        output.push(hex_from_digit(c / 16));
                        output.push(hex_from_digit(c % 16));

                        count += 1;
                    }
                    output += "\")\n";
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
            let mut count = 0;
            mapping.push((start_line, start_column - 1));
            output += "print(\"";

            for c in &input.as_bytes()[start..] {
                if count == 10000 {
                    mapping.push((start_line, start_column - 1));

                    output += "\")\n";
                    output += "print(\"";

                    count = 0;
                }

                output += "\\x";
                output.push(hex_from_digit(c / 16));
                output.push(hex_from_digit(c % 16));
                count += 1;
            }
            output += "\")\n";
        }
    }

    assert_eq!(output.lines().count(), mapping.len());

    mapping.push((line, column - 1));

    (output, mapping)
}

fn link_scripts(isolate: &mut v8::Isolate) -> v8::Global<v8::Object> {
    let handle_scope = &mut v8::HandleScope::new(isolate);
    let context = v8::Context::new(handle_scope, Default::default());
    let scope = &mut v8::ContextScope::new(handle_scope, context);

    let imports = v8::Object::new(scope);
    let env = v8::Object::new(scope);

    let name = v8::String::new(scope, "print").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut _rv: v8::ReturnValue| {
            let result = val_to_string(scope, args.get(0));
            let context: &mut Context = scope.get_slot_mut().unwrap();
            context.output.push_str(&result);
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "println").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut _rv: v8::ReturnValue| {
            let result = val_to_string(scope, args.get(0));

            let context: &mut Context = scope.get_slot_mut().unwrap();
            context.output.push_str(&result);
            context.output.push('\n');
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "hash").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let password = val_to_string(scope, args.get(0));
            let result = hash(password, DEFAULT_COST).unwrap();

            rv.set(string_to_val(scope, &result));
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "verify").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let password = val_to_string(scope, args.get(0));
            let hash = val_to_string(scope, args.get(1));
            let result = verify(password, &hash).unwrap();

            rv.set_bool(result);
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "body").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         _args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let context: &mut Context = scope.get_slot_mut().unwrap();

            let input = std::mem::take(&mut context.input);
            let result = string_to_val(scope, &input);
            rv.set(result);

            let context: &mut Context = scope.get_slot_mut().unwrap();
            context.input = input;
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "urlEncode").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let input = val_to_string(scope, args.get(0));
            let output =
                percent_encoding::utf8_percent_encode(&input, NON_ALPHANUMERIC).to_string();

            let result = string_to_val(scope, &output);
            rv.set(result);
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "urlDecode").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let input = val_to_string(scope, args.get(0));
            let output = percent_encoding::percent_decode_str(&input)
                .decode_utf8()
                .unwrap_or("".into())
                .replace("+", " ");

            let result = string_to_val(scope, &output);
            rv.set(result);
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "markdown").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let input = val_to_string(scope, args.get(0));
            let output = markdown::to_html_with_options(
                &input,
                &Options {
                    compile: CompileOptions {
                        allow_dangerous_html: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap_or("".to_owned());

            let result = string_to_val(scope, &output);
            rv.set(result);
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "query").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         _args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let context: &Context = scope.get_slot().unwrap();
            let environs = &context.environs;
            let default = String::new();
            let value = environs.get("QUERY_STRING").unwrap_or(&default).clone();
            let result = string_to_val(scope, &value);

            rv.set(result);
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "header").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut _rv: v8::ReturnValue| {
            let header = val_to_string(scope, args.get(0));
            let context: &mut Context = scope.get_slot_mut().unwrap();

            context.headers.push_str(header.trim());
            context.headers.push('\n');
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "cookie").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let name = val_to_string(scope, args.get(0));

            let context: &mut Context = scope.get_slot_mut().unwrap();
            let default = "".to_owned();
            let cookie = context.environs.get("HTTP_COOKIE").unwrap_or(&default);

            if let Some(mut start) = cookie.find(&(name.clone() + "=")) {
                start += name.len() + 1;

                let mut end = start;
                while end < cookie.len() {
                    if cookie.as_bytes()[end] == b';' {
                        break;
                    }

                    end += 1;
                }

                let result = &cookie[start..end].trim().to_owned();
                rv.set(string_to_val(scope, result));
            } else {
                rv.set(string_to_val(scope, &default));
            }
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "uuid").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         _args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let uuid = Uuid::new_v4();
            rv.set(string_to_val(scope, &uuid.as_hyphenated().to_string()));
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "date").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let epoch = args.get(0).int32_value(scope).unwrap();
            let format = val_to_string(scope, args.get(1));

            let datetime = DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(epoch as u64));
            let result = datetime.format(&format).to_string();

            rv.set(string_to_val(scope, &result));
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "now").unwrap();
    let func = v8::Function::new(
        scope,
        |_scope: &mut v8::HandleScope,
         _args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            rv.set_uint32(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as u32,
            );
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "getEnviron").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let key = val_to_string(scope, args.get(0));
            let default = String::new();
            let context: &mut Context = scope.get_slot_mut().unwrap();
            let value = context.environs.get(&key).unwrap_or(&default).clone();

            rv.set(string_to_val(scope, &value));
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteOpen").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let path = val_to_string(scope, args.get(0));
            let connection = Connection::open_thread_safe(path);

            if connection.is_err() {
                rv.set_null();
            } else {
                let result = extern_to_val(scope, connection.unwrap());
                rv.set(result);
            }
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteExecute").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let connection: &ConnectionThreadSafe = val_to_extern(args.get(0));
            let query = val_to_string(scope, args.get(1));
            let result = connection.execute(&query);

            rv.set_bool(result.is_ok());
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqlitePrepare").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let connection: &ConnectionThreadSafe = val_to_extern(args.get(0));
            let query = val_to_string(scope, args.get(1));

            let statement = connection.prepare(query);

            if statement.is_err() {
                rv.set_null();
            } else {
                let result = extern_to_val(scope, statement.unwrap());
                rv.set(result);
            }
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteBind<string>").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));
            let index = args.get(1).int32_value(scope).unwrap();
            let value = val_to_string(scope, args.get(2));

            let statement = statement.bind((index as usize, value.as_str()));
            rv.set_bool(statement.is_err());
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteBind<int>").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));
            let index = args.get(1).int32_value(scope).unwrap();
            let value = args.get(2).int32_value(scope).unwrap();

            let statement = statement.bind((index as usize, value as i64));
            rv.set_bool(statement.is_err());
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteBind<float>").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));
            let index = args.get(1).int32_value(scope).unwrap();
            let value = args.get(2).number_value(scope).unwrap();

            let statement = statement.bind((index as usize, value));
            rv.set_bool(statement.is_err());
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteBind<bool>").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));
            let index = args.get(1).int32_value(scope).unwrap();
            let value = args.get(2).number_value(scope).unwrap();

            let statement = statement.bind((index as usize, value));
            rv.set_bool(statement.is_err());
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteBindNull").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));
            let index = args.get(1).int32_value(scope).unwrap();

            let statement = statement.bind((index as usize, sqlite::Value::Null));
            rv.set_bool(statement.is_err());
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteNext").unwrap();
    let func = v8::Function::new(
        scope,
        |_scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));

            let state = statement.next();

            match state {
                Ok(state) => {
                    if state == State::Row {
                        rv.set_int32(1);
                    } else {
                        rv.set_int32(0);
                    }
                }
                Err(_) => {
                    rv.set_int32(0);
                }
            }
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteRead<string>").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));
            let value = val_to_string(scope, args.get(1));

            let result: String = statement.read(value.as_str()).unwrap_or_default();
            rv.set(string_to_val(scope, &result));
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteRead<int>").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));
            let value = val_to_string(scope, args.get(1));

            let result: i64 = statement.read(value.as_str()).unwrap_or_default();
            rv.set_int32(result as i32);
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteRead<float>").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));
            let value = val_to_string(scope, args.get(1));

            let result: f64 = statement.read(value.as_str()).unwrap_or_default();
            rv.set_double(result);
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteRead<float>").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));
            let value = val_to_string(scope, args.get(1));

            let result: f64 = statement.read(value.as_str()).unwrap_or_default();
            rv.set_double(result);
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let name = v8::String::new(scope, "sqliteReadNull").unwrap();
    let func = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let statement: &mut Statement = val_to_extern(args.get(0));
            let value = val_to_string(scope, args.get(1));

            let result: Value = statement.read(value.as_str()).unwrap_or_default();
            rv.set_bool(result == Value::Null);
        },
    )
    .unwrap();
    env.set(scope, name.into(), func.into()).unwrap();

    let import_name = v8::String::new(scope, "env").unwrap();
    imports.set(scope, import_name.into(), env.into()).unwrap();

    v8::Global::new(scope, imports)
}

fn run_script(
    req: &mut Request,
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    instance: v8::Local<'_, v8::Function>,
    module: v8::Local<'_, v8::WasmModuleObject>,
    imports: v8::Local<'_, v8::Object>,
    instant: Instant,
) {
    let exports_name = v8::String::new(scope, "exports")
        .unwrap()
        .cast::<v8::Value>();
    let exports = instance
        .new_instance(scope, &[module.into(), imports.into()])
        .unwrap()
        .get(scope, exports_name)
        .unwrap()
        .cast::<v8::Object>();

    let to_string_name = v8::String::new(scope, "toString").unwrap();
    let to_string = exports
        .get(scope, to_string_name.into())
        .unwrap()
        .cast::<v8::Function>();
    let to_string = v8::Global::new(scope, to_string);

    let from_string_name = v8::String::new(scope, "fromString").unwrap();
    let from_string = exports
        .get(scope, from_string_name.into())
        .unwrap()
        .cast::<v8::Function>();
    let from_string = v8::Global::new(scope, from_string);

    let memory_name = v8::String::new(scope, "memory").unwrap();
    let memory = exports
        .get(scope, memory_name.into())
        .unwrap()
        .cast::<v8::WasmMemoryObject>();

    let buffer = memory.buffer();
    let backing = buffer.get_backing_store();

    let environs = req.params();
    let headers = String::new();
    let output = String::new();
    let externals = Vec::new();
    let mut input = String::new();
    req.stdin().read_to_string(&mut input).unwrap();

    let context = Context {
        headers,
        input,
        output,
        environs,
        backing,
        externals,
        from_string,
        to_string,
    };

    scope.set_slot(context);

    let func_name = v8::String::new(scope, "<start>")
        .unwrap()
        .cast::<v8::Value>();
    exports
        .get(scope, func_name)
        .unwrap()
        .cast::<v8::Function>()
        .call(scope, exports.into(), &[]);

    let mut context: Context = scope.remove_slot().unwrap();
    if !context.headers.contains("Content-Type:") {
        context
            .headers
            .push_str("Content-Type: text/html; charset=UTF-8\n");
    }

    let mut result = String::with_capacity(context.headers.len() + context.output.len() + 1024);
    result.push_str(&format!("Interval: {:?}", instant.elapsed()));
    result.push('\n');
    result.push_str(&context.headers);
    result.push('\n');
    result.push_str(&context.output);

    req.stdout().write_all(result.as_bytes()).unwrap();
    scope.request_garbage_collection_for_testing(v8::GarbageCollectionType::Full);
}

fn request(
    mut req: Request,
    isolate: &mut v8::Isolate,
    scripts: &mut HashMap<String, Script>,
    imports: v8::Global<v8::Object>,
) {
    let instant = Instant::now();
    let handle_scope = &mut v8::HandleScope::new(isolate);
    let context = v8::Context::new(handle_scope, Default::default());
    let scope = &mut v8::ContextScope::new(handle_scope, context);
    let imports = v8::Local::new(scope, imports);

    let result = catch_unwind(AssertUnwindSafe(|| {
        let Some(path) = req.param("SCRIPT_FILENAME") else {
            panic!("Missing 'SCRIPT_FILENAME' environment variable")
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

        let script = scripts.get(&path);
        if script.is_none()
            || script
                .as_ref()
                .unwrap()
                .modified
                .ne(&metadata.modified().unwrap())
        {
            let mut child = Command::new(args().nth(1).unwrap())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();

            let mut stdin = child.stdin.take().unwrap();
            let (input, mapping) = read_script(&path);
            stdin.write_all(input.as_bytes()).unwrap();
            drop(stdin);

            let status = child.wait_with_output().unwrap();
            let output = status.stdout;

            let errors = String::from_utf8_lossy(&status.stderr);
            if !errors.is_empty() {
                let re = Regex::new(r"\(null\):([0-9]+):([0-9]+)-([0-9]+):([0-9]+): error: (.*)")
                    .unwrap();

                let mut result = String::new();

                for caps in re.captures_iter(&errors) {
                    let (_, [start_line, start_column, end_line, end_column, message]) =
                        caps.extract();

                    let start_line = start_line.parse::<usize>().unwrap();
                    let start_column = start_column.parse::<i32>().unwrap();
                    let end_line = end_line.parse::<usize>().unwrap();
                    let end_column = end_column.parse::<i32>().unwrap();

                    result.push_str(&format!(
                        "{}:{}:{}-{}:{}: {}\n",
                        path,
                        mapping[start_line - 1].0,
                        mapping[start_line - 1].1 + start_column,
                        mapping[end_line - 1].0,
                        mapping[end_line - 1].1 + end_column,
                        message
                    ));
                }

                panic!("{}", result);
            }

            let module = v8::WasmModuleObject::compile(scope, &output).unwrap();

            let web_assembly_name = v8::String::new(scope, "WebAssembly")
                .unwrap()
                .cast::<v8::Value>();
            let instance_name = v8::String::new(scope, "Instance")
                .unwrap()
                .cast::<v8::Value>();
            let instance = scope
                .get_current_context()
                .global(scope)
                .get(scope, web_assembly_name)
                .unwrap()
                .cast::<v8::Object>()
                .get(scope, instance_name)
                .unwrap()
                .cast::<v8::Function>();

            run_script(&mut req, scope, instance, module, imports, instant);

            let script = Script {
                modified: metadata.modified().unwrap(),
                instance: v8::Global::new(scope, instance),
                module: v8::Global::new(scope, module),
            };

            scripts.insert(path, script);
        } else {
            let script = script.unwrap();
            let instance = v8::Local::new(scope, script.instance.clone());
            let module = v8::Local::new(scope, script.module.clone());
            let imports = v8::Local::new(scope, imports);

            run_script(&mut req, scope, instance, module, imports, instant);
        }
    }));

    if let Err(error) = result {
        let reason = match error.downcast::<String>() {
            Ok(s) => *s,
            Err(e) => match e.downcast::<&'static str>() {
                Ok(s) => (*s).to_string(),
                Err(_) => "Internal Server Error".to_owned(),
            },
        };

        write!(
            &mut req.stdout(),
            "Status: 500 Internal Server Error\n\n{}",
            reason,
        )
        .unwrap_or(());
    }
}

fn main() -> ExitCode {
    if env::args().count() < 2 {
        println!("usage: cyth-cgi <cyth executable> [listen address]");
        return ExitCode::FAILURE;
    }

    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::set_flags_from_string("--expose-gc");
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();

    let transport;
    let listener;

    #[cfg(unix)]
    {
        if env::args().count() > 2 {
            listener = TcpListener::bind(args().nth(2).unwrap()).unwrap();
            transport = fastcgi::unix::Transport::from_raw_fd(listener.as_raw_fd());
        } else {
            transport = fastcgi::unix::Transport::new();
        }
    }

    #[cfg(windows)]
    {
        if env::args().count() > 2 {
            listener = TcpListener::bind(args().nth(2).unwrap()).unwrap();
            transport = fastcgi::windows::Transport::from_tcp(&listener);
        } else {
            panic!("Unix sockets are not supported on this platform");
        }
    }

    for i in 0..num_cpus::get_physical() {
        let thread = thread::spawn(move || {
            let mut scripts = HashMap::<String, Script>::new();
            let mut isolate = v8::Isolate::new(v8::CreateParams::default());
            let imports = link_scripts(&mut isolate);

            fastcgi::run(
                |req| request(req, &mut isolate, &mut scripts, imports.clone()),
                transport,
            );
        });

        if i == num_cpus::get_physical() - 1 {
            thread.join().unwrap();
        }
    }

    unsafe {
        v8::V8::dispose();
    }
    v8::V8::dispose_platform();

    ExitCode::SUCCESS
}
