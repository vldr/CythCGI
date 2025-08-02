extern crate fastcgi;

use std::{
    cmp,
    collections::HashMap,
    fs,
    io::Write,
    net::TcpListener,
    process::{Command, Stdio},
    sync::Mutex,
    time::SystemTime,
};

use fastcgi::Request;
use wasmtime::{
    AsContext, AsContextMut, Caller, Config, Engine, Func, FuncType, Instance, Module, Store,
    TypedFunc,
};

struct Script {
    modified: SystemTime,
    store: Store<String>,
    start: TypedFunc<(), ()>,
}

const IMPORTS: &'static [u8] = b"import \"env\"
    void print(string n)
";

fn dedent(input: String) -> String {
    let lines: Vec<&str> = input.lines().collect();
    if lines.is_empty() {
        return "".to_owned();
    }

    let min_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);

    let mut result = String::new();
    let mut first = true;

    for line in lines {
        if !first {
            result.push('\n');
        }
        first = false;

        if line.trim().is_empty() {
            continue;
        }

        let start = cmp::min(min_indent, line.len() - line.trim_start().len());
        result.push_str(&line[start..]);
    }

    result.trim_matches('\n').to_owned()
}

fn process_request(mut req: Request, engine: &Engine, scripts: &Mutex<HashMap<String, Script>>) {
    use std::fmt::Write;

    let mut scripts = scripts.lock().unwrap_or_else(|error| error.into_inner());

    let Some(path) = req.param("SCRIPT_FILENAME") else {
        write!(&mut req.stdout(), "Status: 500 Internal Server Error\n\n").unwrap_or(());
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
        let data = fs::read(&path).unwrap();
        let mut input = Vec::new();
        input.extend_from_slice(IMPORTS);

        let mut start = 0;
        let mut code = false;

        fn hex_from_digit(num: u8) -> u8 {
            if num < 10 {
                b'0' + num
            } else {
                b'A' + num - 10
            }
        }

        for i in 0..data.len() {
            if code {
                if data[i] == b'?' && i + 1 < data.len() && data[i + 1] == b'>' {
                    let a = dedent(String::from_utf8((&data[start..i]).to_vec()).unwrap());
                    input.extend_from_slice(a.as_bytes());

                    start = i + 2;
                    code = false;
                }
            } else {
                if data[i] == b'<' && i + 1 < data.len() && data[i + 1] == b'?' {
                    if i != start {
                        input.extend_from_slice(b"\nprint(\"");
                        for c in &data[start..i] {
                            input.extend_from_slice(b"\\x");
                            input.push(hex_from_digit(c / 16));
                            input.push(hex_from_digit(c % 16));
                        }
                        input.extend_from_slice(b"\")\n");
                    }

                    start = i + 2;
                    code = true;
                }
            }
        }

        if code {
            input.extend_from_slice(&data[start..]);
        } else {
            input.extend_from_slice(b"\nprint(\"");
            for c in &data[start..] {
                input.extend_from_slice(b"\\x");
                input.push(hex_from_digit(c / 16));
                input.push(hex_from_digit(c % 16));
            }
            input.extend_from_slice(b"\")\n");
        }

        let mut child = Command::new("./cyth")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&input).unwrap();
        drop(stdin);

        let status = child.wait_with_output().unwrap();
        let output = status.stdout;

        let errors = String::from_utf8_lossy(&status.stderr);
        if errors.len() > 0 {
            write!(
                &mut req.stdout(),
                "Status: 500 Internal Server Error\n\n{}",
                errors
            )
            .unwrap_or(());
            return;
        }

        let mut store = Store::new(&engine, String::new());
        let module = Module::from_binary(&engine, &output).unwrap();

        let a = module.imports().find(|a| a.name() == "print").unwrap();

        let func_ty = FuncType::new(&engine, a.ty().unwrap_func().params(), []);
        let func = Func::new(&mut store, func_ty, |mut poop, params, _results| {
            let a = params.get(0).unwrap().unwrap_any_ref().unwrap();
            let a = a.as_array(poop.as_context()).unwrap().unwrap();

            let mut result = String::new();

            for p in a.elems(poop.as_context_mut()).unwrap() {
                let int_val = p.i32().unwrap();
                let ch = std::char::from_u32(int_val as u32).unwrap();

                result.push(ch);
            }

            write!(poop.data_mut(), "{}", result).unwrap();

            Ok(())
        });

        let instance = Instance::new(&mut store, &module, &[func.into()]).unwrap();
        let start = instance
            .get_typed_func::<(), ()>(&mut store, "<start>")
            .unwrap();

        let mut script = Script {
            modified: metadata.modified().unwrap(),
            store,
            start,
        };

        script.store.data_mut().clear();
        script.start.call(&mut script.store, ()).unwrap();

        println!("Compiling and running {}", path);

        write!(
            &mut req.stdout(),
            "Content-Type: text/plain\n\n{}",
            script.store.data()
        )
        .unwrap_or(());

        scripts.insert(path, script);
    } else {
        println!("Caching {}", path);

        let script = script.unwrap();
        script.store.data_mut().clear();
        script.start.call(&mut script.store, ()).unwrap();

        write!(
            &mut req.stdout(),
            "Content-Type: text/plain\n\n{}",
            script.store.data()
        )
        .unwrap_or(());
    }
}

fn main() {
    let mut config = Config::new();
    config.wasm_gc(true);
    config.wasm_reference_types(true);
    config.wasm_function_references(true);

    let engine = Engine::new(&config).unwrap();

    let listener = TcpListener::bind("127.0.0.1:1237").unwrap();
    let scripts = Mutex::new(HashMap::<String, Script>::new());

    fastcgi::run_tcp(
        move |req| process_request(req, &engine, &scripts),
        &listener,
    );
}
