extern crate fastcgi;

use std::{
    cmp,
    collections::HashMap,
    env::{self, args},
    fs,
    io::{Read, Write},
    net::TcpListener,
    panic::{AssertUnwindSafe, catch_unwind},
    process::{Command, ExitCode, Stdio},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bcrypt::{DEFAULT_COST, hash, verify};
use chrono::{DateTime, Local};
use markdown::{CompileOptions, Options};
use percent_encoding::NON_ALPHANUMERIC;
use regex::Regex;

use dashmap::DashMap;
use fastcgi::Request;
use sqlite::{Connection, ConnectionThreadSafe, State, Statement, Value};
use uuid::Uuid;
use wasmtime::{
    AnyRef, ArrayRef, ArrayRefPre, ArrayType, AsContext, AsContextMut, Caller, Config, Engine,
    ExternRef, FieldType, HeapType, InstanceAllocationStrategy, InstancePre, Linker, Module,
    Mutability, PoolingAllocationConfig, RefType, StorageType, Store, StructRef, StructRefPre,
    StructType, Val, ValType,
};

struct Script {
    modified: SystemTime,
    instance_pre: InstancePre<Context>,
}

#[derive(Default)]
struct Context {
    input: String,
    output: String,
    headers: String,
    environs: Arc<HashMap<String, String>>,
}

const IMPORTS: &str = "import \"env\"
    void print(string a)
    void println(string a)
    void printBuffer(char[] a)

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

    any sqliteOpen(string a)
    bool sqliteExecute(any a, string b)
    any sqlitePrepare(any a, string b)
    bool sqliteBind<T>(any a, int b, T c)
    bool sqliteBindNull(any a, int b)
    bool sqliteNext(any a)
    bool sqliteReadNull(any a, string b)
    T sqliteRead<T>(any a, string b)

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

fn string_array_to_val(caller: &mut Caller<'_, Context>, buf: &Vec<&String>) -> Val {
    let string_ty = ArrayType::new(
        caller.engine(),
        FieldType::new(Mutability::Var, StorageType::I8),
    );
    let string_ref_ty = RefType::new(false, HeapType::ConcreteArray(string_ty.clone()));

    let array_ty = ArrayType::new(
        caller.engine(),
        FieldType::new(Mutability::Var, ValType::Ref(string_ref_ty.clone()).into()),
    );
    let array_ref_ty = RefType::new(true, HeapType::ConcreteArray(array_ty.clone()));

    let struct_ty = StructType::new(
        caller.engine(),
        [
            FieldType::new(Mutability::Var, ValType::Ref(array_ref_ty).into()),
            FieldType::new(Mutability::Var, ValType::I32.into()),
        ],
    )
    .unwrap();

    let struct_allocator = StructRefPre::new(caller.as_context_mut(), struct_ty);
    let array_allocator = ArrayRefPre::new(caller.as_context_mut(), array_ty);

    let list_len = buf.len();
    let mut list = Vec::<Val>::with_capacity(list_len);

    for string in buf {
        list.push(string_to_val(caller, &string));
    }

    let array = ArrayRef::new_fixed(caller.as_context_mut(), &array_allocator, &list).unwrap();
    let class = StructRef::new(
        caller.as_context_mut(),
        &struct_allocator,
        &[array.into(), Val::I32(list_len as i32)],
    )
    .unwrap();

    Val::AnyRef(Some(class.to_anyref()))
}

fn val_to_char_array(caller: &mut Caller<'_, Context>, val: &Val) -> Vec<u8> {
    let class = val.unwrap_any_ref().unwrap();
    let class = class.unwrap_struct(caller.as_context()).unwrap();

    let size = class.field(caller.as_context_mut(), 1).unwrap();
    let size = size.unwrap_i32();

    let array = class.field(caller.as_context_mut(), 0).unwrap();
    let array = array.unwrap_any_ref().unwrap();
    let array = array.as_array(caller.as_context()).unwrap().unwrap();

    let mut result = Vec::with_capacity(size as usize);

    for elem in array
        .elems(caller.as_context_mut())
        .unwrap()
        .take(size as usize)
    {
        result.push(elem.unwrap_i32() as u8);
    }

    result
}

fn char_array_to_val(caller: &mut Caller<'_, Context>, buf: Vec<u8>) -> Val {
    let array_ty = ArrayType::new(
        caller.engine(),
        FieldType::new(Mutability::Var, StorageType::I8),
    );

    let array_ref_ty = RefType::new(true, HeapType::ConcreteArray(array_ty.clone()));

    let struct_ty = StructType::new(
        caller.engine(),
        [
            FieldType::new(Mutability::Var, ValType::Ref(array_ref_ty).into()),
            FieldType::new(Mutability::Var, ValType::I32.into()),
        ],
    )
    .unwrap();

    let struct_allocator = StructRefPre::new(caller.as_context_mut(), struct_ty);
    let array_allocator = ArrayRefPre::new(caller.as_context_mut(), array_ty);

    let mut list = Vec::<Val>::with_capacity(buf.len());
    for byte in &buf {
        list.push(Val::I32(*byte as i32));
    }

    let array = ArrayRef::new_fixed(caller.as_context_mut(), &array_allocator, &list).unwrap();
    let class = StructRef::new(
        caller.as_context_mut(),
        &struct_allocator,
        &[array.into(), Val::I32(buf.len() as i32)],
    )
    .unwrap();

    Val::AnyRef(Some(class.to_anyref()))
}

fn val_to_string(caller: &mut Caller<'_, Context>, val: &Val) -> String {
    let array = val.unwrap_any_ref().unwrap();
    let array = array.as_array(caller.as_context()).unwrap().unwrap();

    let mut result = Vec::with_capacity(array.len(caller.as_context()).unwrap() as usize);
    for elem in array.elems(caller.as_context_mut()).unwrap() {
        result.push(elem.unwrap_i32() as u8);
    }

    unsafe { String::from_utf8_unchecked(result) }
}

fn val_to_externref<'a, T: 'static>(
    caller: &'a mut Caller<'_, Context>,
    val: &'a Val,
) -> &'a mut T {
    let any_ref = val.unwrap_any_ref().unwrap();
    let extern_ref = ExternRef::convert_any(caller.as_context_mut(), *any_ref).unwrap();
    let data = extern_ref
        .data_mut(caller.as_context_mut())
        .unwrap()
        .unwrap();

    let data = data.downcast_mut::<T>().unwrap();

    data
}

fn string_to_val(caller: &mut Caller<'_, Context>, string: &str) -> Val {
    let array_ty = ArrayType::new(
        caller.engine(),
        FieldType::new(Mutability::Var, StorageType::I8),
    );

    let allocator = ArrayRefPre::new(caller.as_context_mut(), array_ty);

    let mut list = Vec::<Val>::with_capacity(string.len());
    for byte in string.as_bytes() {
        list.push(Val::I32(*byte as i32));
    }

    let array = ArrayRef::new_fixed(caller.as_context_mut(), &allocator, &list).unwrap();

    Val::AnyRef(Some(array.to_anyref()))
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
                    mapping.push((start_line, start_column - 1));

                    output += "print(\"";
                    for c in &input.as_bytes()[start..i] {
                        output += "\\x";
                        output.push(hex_from_digit(c / 16));
                        output.push(hex_from_digit(c % 16));
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
            mapping.push((start_line, start_column - 1));

            output += "print(\"";
            for c in &input.as_bytes()[start..] {
                output += "\\x";
                output.push(hex_from_digit(c / 16));
                output.push(hex_from_digit(c % 16));
            }
            output += "\")\n";
        }
    }

    assert_eq!(output.lines().count(), mapping.len());

    mapping.push((line, column - 1));

    (output, mapping)
}

fn link_script(engine: &Engine, module: &Module) -> InstancePre<Context> {
    let mut linker = Linker::<Context>::new(engine);

    if let Some(func_ty) = module.imports().find(|func| func.name() == "print") {
        linker
            .func_new(
                "env",
                "print",
                func_ty.ty().func().unwrap().clone(),
                |mut caller, params, _results| {
                    let result = val_to_string(&mut caller, params.get(0).unwrap());
                    caller.data_mut().output.push_str(&result);

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "println") {
        linker
            .func_new(
                "env",
                "println",
                func_ty.ty().func().unwrap().clone(),
                |mut caller, params, _results| {
                    let result = val_to_string(&mut caller, params.get(0).unwrap());
                    caller.data_mut().output.push_str(&result);
                    caller.data_mut().output.push('\n');

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "printBuffer") {
        linker
            .func_new(
                "env",
                "printBuffer",
                func_ty.ty().func().unwrap().clone(),
                |mut caller, params, _results| {
                    let result = val_to_char_array(&mut caller, params.get(0).unwrap());
                    let result = unsafe { String::from_utf8_unchecked(result) };
                    caller.data_mut().output.push_str(&result);

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "hash") {
        linker
            .func_new(
                "env",
                "hash",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let password = val_to_string(&mut caller, params.get(0).unwrap());
                    let result = hash(password, DEFAULT_COST).unwrap();

                    results[0] = string_to_val(&mut caller, &result);
                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "verify") {
        linker
            .func_new(
                "env",
                "verify",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let password = val_to_string(&mut caller, params.get(0).unwrap());
                    let hash = val_to_string(&mut caller, params.get(1).unwrap());
                    let result = verify(password, &hash).unwrap();

                    results[0] = Val::I32(result.into());
                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "body") {
        linker
            .func_new(
                "env",
                "body",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, _params, results| {
                    let input = std::mem::take(&mut caller.data_mut().input);
                    let body = string_to_val(&mut caller, &input);

                    caller.data_mut().input = input;
                    results[0] = body;
                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "urlEncode") {
        linker
            .func_new(
                "env",
                "urlEncode",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let input = val_to_string(&mut caller, params.get(0).unwrap());
                    let output =
                        percent_encoding::utf8_percent_encode(&input, NON_ALPHANUMERIC).to_string();

                    results[0] = string_to_val(&mut caller, &output);

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "urlDecode") {
        linker
            .func_new(
                "env",
                "urlDecode",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let input = val_to_string(&mut caller, params.get(0).unwrap());
                    let output = percent_encoding::percent_decode_str(&input)
                        .decode_utf8()
                        .unwrap_or("".into())
                        .into_owned()
                        .replace("+", " ");

                    results[0] = string_to_val(&mut caller, &output);

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "markdown") {
        linker
            .func_new(
                "env",
                "markdown",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let input = val_to_string(&mut caller, params.get(0).unwrap());
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

                    results[0] = string_to_val(&mut caller, &output);

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "query") {
        linker
            .func_new(
                "env",
                "query",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, _params, results| {
                    let environs = caller.data().environs.clone();
                    let default = String::new();
                    let value = environs.get("QUERY_STRING").unwrap_or(&default);
                    let body = string_to_val(&mut caller, &value);

                    results[0] = body;
                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "header") {
        linker
            .func_new(
                "env",
                "header",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, _results| {
                    let header = val_to_string(&mut caller, params.get(0).unwrap());
                    caller.data_mut().headers.push_str(header.trim());
                    caller.data_mut().headers.push('\n');

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "cookie") {
        linker
            .func_new(
                "env",
                "cookie",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let name = val_to_string(&mut caller, params.get(0).unwrap());
                    let empty_string = "".to_owned();
                    let cookie = caller
                        .data()
                        .environs
                        .get("HTTP_COOKIE")
                        .unwrap_or(&empty_string);

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
                        results[0] = string_to_val(&mut caller, result);
                    } else {
                        results[0] = string_to_val(&mut caller, &empty_string);
                    }

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "uuid") {
        linker
            .func_new(
                "env",
                "uuid",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, _params, results| {
                    let uuid = Uuid::new_v4();
                    results[0] = string_to_val(&mut caller, &uuid.as_hyphenated().to_string());

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "date") {
        linker
            .func_new(
                "env",
                "date",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let epoch = params.get(0).unwrap().unwrap_i32();
                    let format = val_to_string(&mut caller, params.get(1).unwrap());

                    let datetime =
                        DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(epoch as u64));
                    let result = datetime.format(&format).to_string();

                    results[0] = string_to_val(&mut caller, &result);

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "now") {
        linker
            .func_new(
                "env",
                "now",
                func_ty.ty().func().unwrap().clone(),
                move |mut _caller, _params, results| {
                    results[0] = Val::I32(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i32,
                    );

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "getEnviron") {
        linker
            .func_new(
                "env",
                "getEnviron",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let environs = caller.data().environs.clone();
                    let key = val_to_string(&mut caller, params.get(0).unwrap());
                    let default = String::new();
                    let value = environs.get(&key).unwrap_or(&default);

                    results[0] = string_to_val(&mut caller, &value);
                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "getEnvirons") {
        linker
            .func_new(
                "env",
                "getEnvirons",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, _params, results| {
                    let environs = caller.data().environs.clone();
                    let list: Vec<&String> = environs.keys().collect();

                    results[0] = string_array_to_val(&mut caller, &list);
                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "sqliteOpen") {
        linker
            .func_new(
                "env",
                "sqliteOpen",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let path = val_to_string(&mut caller, params.get(0).unwrap());
                    let connection = Connection::open_thread_safe(path);

                    if connection.is_err() {
                        results[0] = Val::null_any_ref();
                    } else {
                        let extern_ref =
                            ExternRef::new(caller.as_context_mut(), connection.unwrap()).unwrap();
                        let any_ref =
                            AnyRef::convert_extern(caller.as_context_mut(), extern_ref).unwrap();

                        results[0] = Val::AnyRef(Some(any_ref));
                    }

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "sqliteExecute") {
        linker
            .func_new(
                "env",
                "sqliteExecute",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let query = val_to_string(&mut caller, params.get(1).unwrap());
                    let connection: &ConnectionThreadSafe =
                        val_to_externref(&mut caller, params.get(0).unwrap());
                    let result = connection.execute(&query);

                    results[0] = Val::I32(result.is_ok().into());

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "sqlitePrepare") {
        linker
            .func_new(
                "env",
                "sqlitePrepare",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller: Caller<'_, Context>, params, results| {
                    let query = val_to_string(&mut caller, params.get(1).unwrap());
                    let connection: &ConnectionThreadSafe =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let statement = connection.prepare(query);

                    if statement.is_err() {
                        results[0] = Val::null_any_ref();
                    } else {
                        let extern_ref =
                            ExternRef::new(caller.as_context_mut(), statement.unwrap()).unwrap();
                        let any_ref =
                            AnyRef::convert_extern(caller.as_context_mut(), extern_ref).unwrap();

                        results[0] = Val::AnyRef(Some(any_ref));
                    }

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module
        .imports()
        .find(|func| func.name() == "sqliteBind<string>")
    {
        linker
            .func_new(
                "env",
                "sqliteBind<string>",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let index = params.get(1).unwrap().unwrap_i32();
                    let value = val_to_string(&mut caller, params.get(2).unwrap());
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let result = statement.bind((index as usize, value.as_str()));

                    results[0] = Val::I32(result.is_ok().into());

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module
        .imports()
        .find(|func| func.name() == "sqliteBind<int>")
    {
        linker
            .func_new(
                "env",
                "sqliteBind<int>",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let index = params.get(1).unwrap().unwrap_i32();
                    let value = params.get(2).unwrap().unwrap_i32();
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let result = statement.bind((index as usize, value as i64));

                    results[0] = Val::I32(result.is_ok().into());

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module
        .imports()
        .find(|func| func.name() == "sqliteBind<float>")
    {
        linker
            .func_new(
                "env",
                "sqliteBind<float>",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let index = params.get(1).unwrap().unwrap_i32();
                    let value = params.get(2).unwrap().unwrap_f32();
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let result = statement.bind((index as usize, value as f64));

                    results[0] = Val::I32(result.is_ok().into());

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module
        .imports()
        .find(|func| func.name() == "sqliteBind<char[]>")
    {
        linker
            .func_new(
                "env",
                "sqliteBind<char[]>",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let index = params.get(1).unwrap().unwrap_i32();
                    let value = val_to_char_array(&mut caller, params.get(2).unwrap());
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let result = statement.bind((index as usize, value.as_slice()));

                    results[0] = Val::I32(result.is_ok().into());

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module
        .imports()
        .find(|func| func.name() == "sqliteBindNull")
    {
        linker
            .func_new(
                "env",
                "sqliteBindNull",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let index = params.get(1).unwrap().unwrap_i32();
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let result = statement.bind((index as usize, sqlite::Value::Null));

                    results[0] = Val::I32(result.is_ok().into());

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module.imports().find(|func| func.name() == "sqliteNext") {
        linker
            .func_new(
                "env",
                "sqliteNext",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let state = statement.next();

                    match state {
                        Ok(state) => {
                            if state == State::Row {
                                results[0] = Val::I32(1);
                            } else {
                                results[0] = Val::I32(0);
                            }
                        }
                        Err(_) => {
                            results[0] = Val::I32(0);
                        }
                    }

                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module
        .imports()
        .find(|func| func.name() == "sqliteRead<string>")
    {
        linker
            .func_new(
                "env",
                "sqliteRead<string>",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let value = val_to_string(&mut caller, params.get(1).unwrap());
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let result: String = statement.read(value.as_str()).unwrap_or_default();

                    results[0] = string_to_val(&mut caller, &result);
                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module
        .imports()
        .find(|func| func.name() == "sqliteRead<int>")
    {
        linker
            .func_new(
                "env",
                "sqliteRead<int>",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let value = val_to_string(&mut caller, params.get(1).unwrap());
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let result: i64 = statement.read(value.as_str()).unwrap_or_default();

                    results[0] = Val::I32(result as i32);
                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module
        .imports()
        .find(|func| func.name() == "sqliteRead<float>")
    {
        linker
            .func_new(
                "env",
                "sqliteRead<float>",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let value = val_to_string(&mut caller, params.get(1).unwrap());
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let result: f64 = statement.read(value.as_str()).unwrap_or_default();

                    results[0] = Val::F32(f32::to_bits(result as f32));
                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module
        .imports()
        .find(|func| func.name() == "sqliteRead<char[]>")
    {
        linker
            .func_new(
                "env",
                "sqliteRead<char[]>",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let value = val_to_string(&mut caller, params.get(1).unwrap());
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let result: Vec<u8> = statement.read(value.as_str()).unwrap_or_default();

                    results[0] = char_array_to_val(&mut caller, result);
                    Ok(())
                },
            )
            .unwrap();
    }

    if let Some(func_ty) = module
        .imports()
        .find(|func| func.name() == "sqliteReadNull")
    {
        linker
            .func_new(
                "env",
                "sqliteReadNull",
                func_ty.ty().func().unwrap().clone(),
                move |mut caller, params, results| {
                    let value = val_to_string(&mut caller, params.get(1).unwrap());
                    let statement: &mut Statement =
                        val_to_externref(&mut caller, params.get(0).unwrap());

                    let result: Value = statement.read(value.as_str()).unwrap_or_default();

                    results[0] = Val::I32((result == Value::Null) as i32);
                    Ok(())
                },
            )
            .unwrap();
    }

    linker.instantiate_pre(module).unwrap()
}

fn run_script(req: &mut Request, engine: &Engine, instance_pre: &InstancePre<Context>) {
    let instant = Instant::now();
    let environs = req.params();
    let headers = String::new();
    let output = String::new();
    let mut input = String::new();
    req.stdin().read_to_string(&mut input).unwrap();

    let mut store = Store::new(
        engine,
        Context {
            headers,
            input,
            output,
            environs,
        },
    );
    let instance = instance_pre.instantiate(&mut store).unwrap();
    let start = instance.get_func(&mut store, "<start>").unwrap();

    unsafe { start.call_unchecked(&mut store, &mut *vec![]).unwrap() };

    if !store.data().headers.contains("Content-Type:") {
        store
            .data_mut()
            .headers
            .push_str("Content-Type: text/html; charset=UTF-8\n");
    }

    write!(
        &mut req.stdout(),
        "Interval: {:?}\n{}\n{}",
        instant.elapsed(),
        store.data().headers,
        store.data().output
    )
    .unwrap_or(());
}

fn request(mut req: Request, engine: &Engine, scripts: &DashMap<String, Script>) {
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
            drop(script);

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

            let module = Module::from_binary(engine, &output).unwrap();
            let instance_pre = link_script(engine, &module);
            run_script(&mut req, engine, &instance_pre);

            let script = Script {
                modified: metadata.modified().unwrap(),
                instance_pre,
            };

            scripts.insert(path, script);
        } else {
            let script = script.unwrap();
            run_script(&mut req, engine, &script.instance_pre);
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

    let mut pool = PoolingAllocationConfig::new();
    pool.total_gc_heaps(10000);
    pool.total_core_instances(10000);

    let mut config = Config::new();
    config.wasm_gc(true);
    config.parallel_compilation(true);
    config.collector(wasmtime::Collector::Null);
    config.wasm_reference_types(true);
    config.wasm_function_references(true);
    config.signals_based_traps(true);
    config.memory_init_cow(true);
    config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));

    unsafe {
        if let Err(error) = config.target("x86_64") {
            eprintln!(
                "Wasmtime was not compiled with the x86_64 backend for \
                     Cranelift enabled: {error:?}",
            );
            return ExitCode::FAILURE;
        }
        config.cranelift_flag_enable("has_sse41");
        config.cranelift_flag_enable("has_avx");
        config.cranelift_flag_enable("has_avx2");
        config.cranelift_flag_enable("has_lzcnt");
    }

    let engine = Engine::new(&config).unwrap();
    let scripts = DashMap::<String, Script>::new();

    if env::args().count() > 2 {
        let listener = TcpListener::bind(args().nth(2).unwrap()).unwrap();
        fastcgi::run_tcp(move |req| request(req, &engine, &scripts), &listener);
    } else {
        #[cfg(unix)]
        {
            fastcgi::run(move |req| request(req, &engine, &scripts));
        }
        #[cfg(not(unix))]
        {
            panic!("Unix sockets are not supported on this platform");
        }
    }

    ExitCode::SUCCESS
}
