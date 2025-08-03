extern crate fastcgi;

use std::{
    cmp, fs,
    io::Write,
    net::TcpListener,
    process::{Command, Stdio},
    time::SystemTime,
};

use dashmap::DashMap;
use fastcgi::Request;
use wasmtime::{
    AsContext, AsContextMut, Config, Engine, InstanceAllocationStrategy, InstancePre, Linker,
    Module, PoolingAllocationConfig, Store,
};

struct Script {
    modified: SystemTime,
    instance_pre: InstancePre<String>,
}

const IMPORTS: &str = "import \"env\"
    void print(string n)
";

fn read_script(path: &String) -> String {
    fn dedent(input: &str) -> String {
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

    let input = fs::read_to_string(&path).unwrap();
    let mut output = String::new();
    output += IMPORTS;

    let mut start = 0;
    let mut code = false;

    fn hex_from_digit(num: u8) -> char {
        if num < 10 {
            (b'0' + num) as char
        } else {
            (b'A' + num - 10) as char
        }
    }

    for i in 0..input.len() {
        if code {
            if input.as_bytes()[i] == b'?' && i + 1 < input.len() && input.as_bytes()[i + 1] == b'>'
            {
                output += &dedent(&input[start..i]);

                start = i + 2;
                code = false;
            }
        } else {
            if input.as_bytes()[i] == b'<' && i + 1 < input.len() && input.as_bytes()[i + 1] == b'?'
            {
                if i != start {
                    output += "\nprint(\"";
                    for c in input[start..i].as_bytes() {
                        output += "\\x";
                        output.push(hex_from_digit(c / 16));
                        output.push(hex_from_digit(c % 16));
                    }
                    output += "\")\n";
                }

                start = i + 2;
                code = true;
            }
        }
    }

    if code {
        output += &dedent(&input[start..]);
    } else {
        output += "\nprint(\"";
        for c in input[start..].as_bytes() {
            output += "\\x";
            output.push(hex_from_digit(c / 16));
            output.push(hex_from_digit(c % 16));
        }
        output += "\")\n";
    }

    output
}

fn link_script(engine: &Engine, module: &Module) -> InstancePre<String> {
    use std::fmt::Write;
    let mut linker = Linker::<String>::new(&engine);

    let func_import = module
        .imports()
        .find(|func| func.name() == "print")
        .unwrap();
    let func_ty = func_import.ty().unwrap_func().clone();
    linker
        .func_new("env", "print", func_ty, |mut caller, params, _results| {
            let array = params.get(0).unwrap().unwrap_any_ref().unwrap();
            let array = array.as_array(caller.as_context()).unwrap().unwrap();

            let mut result =
                String::with_capacity(array.len(caller.as_context()).unwrap() as usize);

            for elem in array.elems(caller.as_context_mut()).unwrap() {
                let ch = std::char::from_u32(elem.i32().unwrap() as u32).unwrap();

                result.push(ch);
            }

            write!(caller.data_mut(), "{}", result).unwrap();

            Ok(())
        })
        .unwrap();

    linker.instantiate_pre(&module).unwrap()
}

fn run_script(req: &mut Request, engine: &Engine, instance_pre: &InstancePre<String>) {
    let mut store = Store::new(engine, String::new());
    let instance = instance_pre.instantiate(&mut store).unwrap();
    let start = instance
        .get_typed_func::<(), ()>(&mut store, "<start>")
        .unwrap();

    start.call(&mut store, ()).unwrap();

    write!(
        &mut req.stdout(),
        "Content-Type: text/html; charset=UTF-8\n\n{}",
        store.data()
    )
    .unwrap_or(());
}

fn request(mut req: Request, engine: &Engine, scripts: &DashMap<String, Script>) {
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

    let script = scripts.get(&path);
    if script.is_none()
        || script
            .as_ref()
            .unwrap()
            .modified
            .ne(&metadata.modified().unwrap())
    {
        drop(script);

        let mut child = Command::new("./cyth")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let mut stdin = child.stdin.take().unwrap();
        let input = read_script(&path);
        stdin.write_all(input.as_bytes()).unwrap();
        drop(stdin);

        let status = child.wait_with_output().unwrap();
        let output = status.stdout;

        let errors = String::from_utf8_lossy(&status.stderr);
        if errors.len() > 0 {
            write!(
                &mut req.stdout(),
                "Status: 500 Internal Server Error\n\n{}",
                errors.replace("(null)", &path)
            )
            .unwrap_or(());
            return;
        }

        let module = Module::from_binary(&engine, &output).unwrap();
        let instance_pre = link_script(&engine, &module);
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
}

fn main() {
    let mut pool = PoolingAllocationConfig::new();
    pool.total_gc_heaps(10000);
    pool.total_core_instances(10000);

    let mut config = Config::new();
    config.wasm_gc(true);
    config.wasm_reference_types(true);
    config.wasm_function_references(true);
    config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));

    let engine = Engine::new(&config).unwrap();

    let listener = TcpListener::bind("127.0.0.1:1237").unwrap();
    let scripts = DashMap::<String, Script>::new();

    fastcgi::run_tcp(move |req| request(req, &engine, &scripts), &listener);
}
