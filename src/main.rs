extern crate fastcgi;

use std::{
    alloc::{Layout, alloc},
    cmp,
    collections::HashMap,
    env::{self, args},
    ffi::{CString, c_char, c_float, c_int, c_void},
    fmt::Write as _,
    fs,
    io::{Read, Write},
    mem::transmute,
    net::TcpListener,
    panic::{AssertUnwindSafe, catch_unwind},
    process::{Command, ExitCode, Stdio},
    ptr,
    rc::Rc,
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
    text: Rc<Vec<String>>,
}

unsafe extern "C" {
    fn jit(source: *const c_char) -> *const c_void;
    fn jit_set_function(jit: *const c_void, name: *const c_char, cb: *const c_void);
    fn jit_generate(jit: *const c_void, logging: c_int);
    fn jit_run(jit: *const c_void);
    fn jit_destroy(jit: *const c_void);
    fn jit_alloc(jit: *const c_void, size: c_int);
}

pub fn cyth_new(s: &str) -> *mut u8 {
    unsafe {
        let len = s.len() as i32;
        let total_size = 4 + s.len();
        let layout = Layout::from_size_align(total_size, 4).unwrap();

        let ptr = alloc(layout);
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }

        ptr::write(ptr as *mut i32, len);
        ptr::copy_nonoverlapping(s.as_ptr(), ptr.add(4), s.len());

        ptr
    }
}

pub unsafe fn cyth_as_str<'a>(ptr: *const u8) -> &'a str {
    let len = unsafe { *(ptr as *const i32) } as usize;
    let data_ptr = unsafe { ptr.add(4) };
    let slice: &[u8] = unsafe { transmute((data_ptr, len)) };
    unsafe { transmute(slice) }
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
}

const IMPORTS: &str = "import \"env\"
    void print(string a)
    void println(string a)
    void printInternal(int index)

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
    string[] getEnvirons()

    string date(int a, string b)
    int now()

    int sqliteOpen(string a)
    bool sqliteExecute(int a, string b)
    int sqlitePrepare(int a, string b)
    bool sqliteBind<T>(int a, int b, T c)
    bool sqliteBindNull(int a, int b)
    bool sqliteNext(int a)
    bool sqliteReadNull(int a, string b)
    T sqliteRead<T>(int a, string b)

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

    T read<T>(string column)
        return sqliteRead<T>(stmt, column)

    bool readNull(string column)
        return sqliteReadNull(stmt, column)

    bool bind(int index, int value)
        return sqliteBind<int>(stmt, index, value)

    bool bind(int index, float value)
        return sqliteBind<float>(stmt, index, value)

    bool bind(int index, string value)
        return sqliteBind<string>(stmt, index, value)
    
    bool bind(int index, char[] value)
        return sqliteBind<char[]>(stmt, index, value)
    
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

fn read_script(path: &String) -> (String, Vec<String>, Vec<(i32, i32)>) {
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
    let mut text = Vec::<String>::new();
    let mut mapping = Vec::<(i32, i32)>::new();
    let mut line = 1;
    let mut column = 1;

    for _ in IMPORTS.lines() {
        mapping.push((0, 0));
    }

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

    (output, text, mapping)
}

fn run_script(req: &mut Request, text: Rc<Vec<String>>) {}

fn request(mut req: Request, scripts: &mut HashMap<String, Script>) {
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

        let script = scripts.get_mut(&path);
        if script.is_none()
            || script
                .as_ref()
                .unwrap()
                .modified
                .ne(&metadata.modified().unwrap())
        {
            let (input, text, mapping) = read_script(&path);

            // let errors = String::from_utf8_lossy(&status.stderr);
            // if !errors.is_empty() {
            //     let re = Regex::new(r"\(null\):([0-9]+):([0-9]+)-([0-9]+):([0-9]+): error: (.*)")
            //         .unwrap();

            //     let mut result = String::new();

            //     for caps in re.captures_iter(&errors) {
            //         let (_, [start_line, start_column, end_line, end_column, message]) =
            //             caps.extract();

            //         let start_line = start_line.parse::<usize>().unwrap();
            //         let start_column = start_column.parse::<i32>().unwrap();
            //         let end_line = end_line.parse::<usize>().unwrap();
            //         let end_column = end_column.parse::<i32>().unwrap();

            //         result.push_str(&format!(
            //             "{}:{}:{}-{}:{}: {}\n",
            //             path,
            //             mapping[start_line - 1].0,
            //             mapping[start_line - 1].1 + start_column,
            //             mapping[end_line - 1].0,
            //             mapping[end_line - 1].1 + end_column,
            //             message
            //         ));
            //     }

            //     panic!("{}", result);
            // }

            let text = Rc::new(text);
            let mut script = Script {
                modified: metadata.modified().unwrap(),
                text: text.clone(),
            };

            unsafe {
                let s = CString::new(input).unwrap();
                let jit = jit(s.as_ptr());

                unsafe extern "C" fn print(input: *const u8) {
                    let context = unsafe { &mut *CONTEXT };
                    context.output.push_str(cyth_as_str(input));
                }

                jit_set_function(
                    jit,
                    CString::new("print").unwrap().as_ptr(),
                    print as *const c_void,
                );

                unsafe extern "C" fn println(input: *const u8) {
                    let context = unsafe { &mut *CONTEXT };
                    context.output.push_str(cyth_as_str(input));
                    context.output.push('\n');
                }

                jit_set_function(
                    jit,
                    CString::new("println").unwrap().as_ptr(),
                    println as *const c_void,
                );

                unsafe extern "C" fn print_internal(n: i32) {
                    let context = unsafe { &mut *CONTEXT };

                    let p = &context.text[n as usize];
                    context.output.write_str(p).unwrap();
                }

                jit_set_function(
                    jit,
                    CString::new("printInternal").unwrap().as_ptr(),
                    print_internal as *const c_void,
                );

                unsafe extern "C" fn url_encode(input: *const u8) -> *const u8 {
                    let input = unsafe { cyth_as_str(input) };
                    let output =
                        percent_encoding::utf8_percent_encode(&input, NON_ALPHANUMERIC).to_string();

                    cyth_new(&output)
                }

                jit_set_function(
                    jit,
                    CString::new("urlEncode").unwrap().as_ptr(),
                    url_encode as *const c_void,
                );
                unsafe extern "C" fn url_decode(input: *const u8) -> *const u8 {
                    let input = unsafe { cyth_as_str(input) };
                    let output = percent_encoding::percent_decode_str(&input)
                        .decode_utf8()
                        .unwrap_or("".into())
                        .into_owned()
                        .replace("+", " ");

                    cyth_new(&output)
                }

                jit_set_function(
                    jit,
                    CString::new("urlDecode").unwrap().as_ptr(),
                    url_decode as *const c_void,
                );
                unsafe extern "C" fn markdown(input: *const u8) -> *const u8 {
                    let input = unsafe { cyth_as_str(input) };
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

                    cyth_new(&output)
                }

                jit_set_function(
                    jit,
                    CString::new("markdown").unwrap().as_ptr(),
                    markdown as *const c_void,
                );
                unsafe extern "C" fn hash(input: *const u8) -> *const u8 {
                    let input = unsafe { cyth_as_str(input) };
                    let output = bcrypt::hash(&input, DEFAULT_COST).unwrap();
                    cyth_new(&output)
                }

                jit_set_function(
                    jit,
                    CString::new("hash").unwrap().as_ptr(),
                    hash as *const c_void,
                );

                unsafe extern "C" fn verify(password: *const u8, hash: *const u8) -> c_int {
                    let password = unsafe { cyth_as_str(password) };
                    let hash = unsafe { cyth_as_str(hash) };
                    let output = bcrypt::verify(&password, &hash).unwrap();

                    output.into()
                }

                jit_set_function(
                    jit,
                    CString::new("verify").unwrap().as_ptr(),
                    verify as *const c_void,
                );
                unsafe extern "C" fn body() -> *const u8 {
                    let context = unsafe { &mut *CONTEXT };

                    cyth_new(&context.input)
                }

                jit_set_function(
                    jit,
                    CString::new("body").unwrap().as_ptr(),
                    body as *const c_void,
                );

                unsafe extern "C" fn query() -> *const u8 {
                    let context = unsafe { &mut *CONTEXT };
                    let default = String::new();
                    let query = context.environs.get("QUERY_STRING").unwrap_or(&default);

                    cyth_new(&query)
                }
                jit_set_function(
                    jit,
                    CString::new("query").unwrap().as_ptr(),
                    query as *const c_void,
                );

                unsafe extern "C" fn header(input: *const u8) {
                    let context = unsafe { &mut *CONTEXT };
                    let input = unsafe { cyth_as_str(input) };

                    context.headers.push_str(input);
                    context.headers.push('\n');
                }

                jit_set_function(
                    jit,
                    CString::new("header").unwrap().as_ptr(),
                    header as *const c_void,
                );
                unsafe extern "C" fn cookie(name: *const u8) -> *const u8 {
                    let context = unsafe { &mut *CONTEXT };
                    let name = unsafe { cyth_as_str(name) };
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
                        return cyth_new(&result);
                    } else {
                        return cyth_new(&empty_string);
                    }
                }

                jit_set_function(
                    jit,
                    CString::new("cookie").unwrap().as_ptr(),
                    cookie as *const c_void,
                );

                unsafe extern "C" fn uuid() -> *const u8 {
                    let uuid = Uuid::new_v4();
                    cyth_new(&uuid.as_hyphenated().to_string())
                }
                jit_set_function(
                    jit,
                    CString::new("uuid").unwrap().as_ptr(),
                    uuid as *const c_void,
                );
                unsafe extern "C" fn get_environ(key: *const u8) -> *const u8 {
                    let context = unsafe { &mut *CONTEXT };
                    let key = unsafe { cyth_as_str(key) };

                    let empty_string = "".to_owned();
                    let environ = context.environs.get(key).unwrap_or(&empty_string);

                    cyth_new(&environ)
                }

                jit_set_function(
                    jit,
                    CString::new("getEnviron").unwrap().as_ptr(),
                    get_environ as *const c_void,
                );
                unsafe extern "C" fn getEnvirons() {
                    let context = unsafe { &mut *CONTEXT };

                    println!("hello {}", context.headers);
                }

                jit_set_function(
                    jit,
                    CString::new("getEnvirons").unwrap().as_ptr(),
                    print as *const c_void,
                );
                unsafe extern "C" fn date(epoch: c_int, format: *const u8) -> *const u8 {
                    let context = unsafe { &mut *CONTEXT };

                    let format = unsafe { cyth_as_str(format) };

                    let datetime =
                        DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(epoch as u64));
                    let result = datetime.format(&format).to_string();

                    cyth_new(&result)
                }

                jit_set_function(
                    jit,
                    CString::new("date").unwrap().as_ptr(),
                    date as *const c_void,
                );

                unsafe extern "C" fn now() -> c_int {
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i32
                }
                jit_set_function(
                    jit,
                    CString::new("now").unwrap().as_ptr(),
                    now as *const c_void,
                );
                unsafe extern "C" fn sqlite_open(path: *const u8) -> c_int {
                    let context = unsafe { &mut *CONTEXT };
                    let path = unsafe { cyth_as_str(path) };
                    let connection = Connection::open_thread_safe(path);

                    if let Ok(connection) = connection {
                        context.connections.push(connection);

                        return context.connections.len() as c_int;
                    }

                    0
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteOpen").unwrap().as_ptr(),
                    sqlite_open as *const c_void,
                );
                unsafe extern "C" fn sqlite_execute(id: c_int, query: *const u8) -> c_int {
                    let context = unsafe { &mut *CONTEXT };
                    let query = unsafe { cyth_as_str(query) };
                    let Some(connection) = context.connections.get_mut((id - 1) as usize) else {
                        return 0;
                    };

                    connection.execute(query).is_ok() as c_int
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteExecute").unwrap().as_ptr(),
                    sqlite_execute as *const c_void,
                );
                unsafe extern "C" fn sqlite_prepare(id: c_int, query: *const u8) -> c_int {
                    let context = unsafe { &mut *CONTEXT };

                    let query = unsafe { cyth_as_str(query) };
                    let Some(connection) = context.connections.get_mut((id - 1) as usize) else {
                        return 0;
                    };

                    let Ok(statement) = connection.prepare(query) else {
                        return 0;
                    };

                    context.statements.push(statement);
                    context.statements.len() as c_int
                }

                jit_set_function(
                    jit,
                    CString::new("sqlitePrepare").unwrap().as_ptr(),
                    sqlite_prepare as *const c_void,
                );
                unsafe extern "C" fn sqlite_bind_int(
                    id: c_int,
                    index: c_int,
                    value: c_int,
                ) -> c_int {
                    let context = unsafe { &mut *CONTEXT };
                    let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                        return 0;
                    };

                    statement.bind((index as usize, value as i64)).is_ok() as c_int
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteBind<int>").unwrap().as_ptr(),
                    sqlite_bind_int as *const c_void,
                );

                unsafe extern "C" fn sqlite_bind_float(
                    id: c_int,
                    index: c_int,
                    value: c_float,
                ) -> c_int {
                    let context = unsafe { &mut *CONTEXT };
                    let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                        return 0;
                    };

                    statement.bind((index as usize, value as f64)).is_ok() as c_int
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteBind<float>").unwrap().as_ptr(),
                    sqlite_bind_float as *const c_void,
                );

                unsafe extern "C" fn sqlite_bind_char() {
                    let context = &mut *CONTEXT;

                    println!("hello {}", context.headers);
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteBind<char[]>").unwrap().as_ptr(),
                    sqlite_bind_char as *const c_void,
                );
                unsafe extern "C" fn sqlite_bind_string(
                    id: c_int,
                    index: c_int,
                    value: *const u8,
                ) -> c_int {
                    let context = unsafe { &mut *CONTEXT };
                    let value = unsafe { cyth_as_str(value) };
                    let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                        return 0;
                    };

                    statement.bind((index as usize, value)).is_ok() as c_int
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteBind<string>").unwrap().as_ptr(),
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

                jit_set_function(
                    jit,
                    CString::new("sqliteBindNull").unwrap().as_ptr(),
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
                                return 1;
                            } else {
                                return 0;
                            }
                        }
                        Err(_) => {
                            return 0;
                        }
                    }
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteNext").unwrap().as_ptr(),
                    sqlite_next as *const c_void,
                );

                unsafe extern "C" fn sqlite_read_int(id: c_int, value: *const u8) -> c_int {
                    let context = unsafe { &mut *CONTEXT };
                    let value = unsafe { cyth_as_str(value) };
                    let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                        return 0;
                    };

                    statement.read::<i64, &str>(value).unwrap_or_default() as c_int
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteRead<int>").unwrap().as_ptr(),
                    sqlite_read_int as *const c_void,
                );

                unsafe extern "C" fn sqlite_read_float(id: c_int, value: *const u8) -> c_float {
                    let context = unsafe { &mut *CONTEXT };
                    let value = unsafe { cyth_as_str(value) };
                    let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                        return 0.0;
                    };

                    statement.read::<f64, &str>(value).unwrap_or_default() as c_float
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteRead<float>").unwrap().as_ptr(),
                    sqlite_read_float as *const c_void,
                );
                unsafe extern "C" fn sqliteReadChar() {
                    let context = &mut *CONTEXT;

                    println!("hello {}", context.headers);
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteRead<char[]>").unwrap().as_ptr(),
                    print as *const c_void,
                );
                unsafe extern "C" fn sqlite_read_string(id: c_int, value: *const u8) -> *const u8 {
                    let context = unsafe { &mut *CONTEXT };
                    let value = unsafe { cyth_as_str(value) };
                    let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                        return cyth_new("");
                    };

                    cyth_new(&statement.read::<String, &str>(value).unwrap_or_default())
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteRead<string>").unwrap().as_ptr(),
                    sqlite_read_string as *const c_void,
                );
                unsafe extern "C" fn sqlite_read_null(id: c_int, value: *const u8) -> c_int {
                    let context = unsafe { &mut *CONTEXT };
                    let value = unsafe { cyth_as_str(value) };
                    let Some(statement) = context.statements.get_mut((id - 1) as usize) else {
                        return 0;
                    };

                    let result: Value = statement.read(value).unwrap_or_default();

                    (result == Value::Null) as c_int
                }

                jit_set_function(
                    jit,
                    CString::new("sqliteReadNull").unwrap().as_ptr(),
                    sqlite_read_null as *const c_void,
                );

                let instant = Instant::now();
                let environs = req.params();
                let headers = String::new();
                let output = String::new();
                let connections = Vec::new();
                let statements = Vec::new();
                let mut input = String::new();
                req.stdin().read_to_string(&mut input).unwrap();

                let mut context = Context {
                    headers,
                    input,
                    output,
                    environs,
                    connections,
                    statements,
                    text: text.clone(),
                };

                CONTEXT = &mut context as *mut Context;

                jit_generate(jit, 0);
                jit_run(jit);
                jit_destroy(jit);

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
            };

            // run_script(
            //     &mut req,
            //     &mut script.store,
            //     &mut script.start,
            //     script.text.clone(),
            // );

            // scripts.insert(path, script);
        } else {
            // let script = script.unwrap();
            // run_script(
            //     &mut req,
            //     &mut script.store,
            //     &mut script.start,
            //     script.text.clone(),
            // );
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

    let mut scripts = HashMap::<String, Script>::new();

    if env::args().count() > 2 {
        let listener = TcpListener::bind(args().nth(2).unwrap()).unwrap();
        fastcgi::run_tcp(move |req| request(req, &mut scripts), &listener);
    } else {
        #[cfg(unix)]
        fastcgi::run(move |req| request(req, &mut scripts));

        #[cfg(windows)]
        {
            println!("error: unix sockets are not supported on this platform");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}
