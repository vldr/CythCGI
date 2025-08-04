extern crate fastcgi;

use std::{
    cmp, fs,
    io::Write,
    net::TcpListener,
    process::{Command, Stdio},
    time::SystemTime,
};

use regex::Regex;

use dashmap::DashMap;
use fastcgi::Request;
use wasmtime::{
    AsContext, AsContextMut, Caller, Config, Engine, InstanceAllocationStrategy, InstancePre,
    Linker, Module, PoolingAllocationConfig, Store, Val,
};

struct Script {
    modified: SystemTime,
    instance_pre: InstancePre<Context>,
}

#[derive(Default)]
struct Context {
    body: String,
}

const IMPORTS: &str = "import \"env\"
    void print(string n)
";

fn val_to_string(caller: &mut Caller<'_, Context>, val: &Val) -> String {
    let array = val.unwrap_any_ref().unwrap();
    let array = array.as_array(caller.as_context()).unwrap().unwrap();

    let mut result = String::with_capacity(array.len(caller.as_context()).unwrap() as usize);

    for elem in array.elems(caller.as_context_mut()).unwrap() {
        let ch = std::char::from_u32(elem.i32().unwrap() as u32).unwrap();
        result.push(ch);
    }

    return result;
}

fn read_script(path: &String) -> (String, Vec<(i32, i32)>) {
    fn dedent(
        input: &str,
        mapping: &mut Vec<(i32, i32)>,
        mut line: i32,
        mut column: i32,
    ) -> String {
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

        for text in lines {
            if text.trim().is_empty() {
                continue;
            }

            let start = cmp::min(min_indent, text.len() - text.trim_start().len());

            mapping.push((line, (column - 1) + start as i32));

            result.push_str(&text[start..]);
            result.push_str(
                format!(" # {} -> {}, {}:{}", mapping.len(), line, column, start).as_str(),
            );
            result.push('\n');

            line += 1;
            column = 1;
        }

        result
    }

    let input = fs::read_to_string(&path).unwrap();
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
    let mut start_line = 0;
    let mut start_column = 0;

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

                start = i + 2;
                code = false;
            }
        } else {
            if input.as_bytes()[i] == b'<' && i + 1 < input.len() && input.as_bytes()[i + 1] == b'?'
            {
                if start < i {
                    mapping.push((line, column));

                    output += "print(\"";
                    for c in input[start..i].as_bytes() {
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
            mapping.push((line, column));

            output += "print(\"";
            for c in input[start..].as_bytes() {
                output += "\\x";
                output.push(hex_from_digit(c / 16));
                output.push(hex_from_digit(c % 16));
            }
            output += "\")\n";
        }
    }

    println!("{}", output);
    // assert_eq!(output.lines().count(), mapping.len());

    (output, mapping)
}

fn link_script(engine: &Engine, module: &Module) -> InstancePre<Context> {
    let mut linker = Linker::<Context>::new(&engine);

    let func_ty = module
        .imports()
        .find(|func| func.name() == "print")
        .unwrap()
        .ty()
        .unwrap_func()
        .clone();

    linker
        .func_new("env", "print", func_ty, |mut caller, params, _results| {
            let result = val_to_string(&mut caller, params.get(0).unwrap());
            caller.data_mut().body.push_str(&result);

            Ok(())
        })
        .unwrap();

    linker.instantiate_pre(&module).unwrap()
}

fn run_script(req: &mut Request, engine: &Engine, instance_pre: &InstancePre<Context>) {
    let mut store = Store::new(engine, Context::default());
    let instance = instance_pre.instantiate(&mut store).unwrap();
    let start = instance
        .get_typed_func::<(), ()>(&mut store, "<start>")
        .unwrap();

    start.call(&mut store, ()).unwrap();

    write!(
        &mut req.stdout(),
        "Content-Type: text/html; charset=UTF-8\n\n{}",
        store.data().body
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
        let (input, mapping) = read_script(&path);
        stdin.write_all(input.as_bytes()).unwrap();
        drop(stdin);

        let status = child.wait_with_output().unwrap();
        let output = status.stdout;

        let errors = String::from_utf8_lossy(&status.stderr);
        if errors.len() > 0 {
            let re =
                Regex::new(r"\(null\):([0-9]+):([0-9]+)-([0-9]+):([0-9]+): error: (.*)").unwrap();

            let mut result = String::new();

            for caps in re.captures_iter(&errors) {
                let (_, [start_line, start_column, end_line, end_column, message]) = caps.extract();

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

            write!(
                &mut req.stdout(),
                "Status: 500 Internal Server Error\n\n{}\n{}",
                result,
                errors
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
