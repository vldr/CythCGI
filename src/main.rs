extern crate fastcgi;

use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::Mutex,
    time::SystemTime,
};

use wasmtime::{Caller, Config, Engine, Func, Instance, Module, Store, TypedFunc};

struct Script {
    modified: SystemTime,
    store: Store<String>,
    start: TypedFunc<(), ()>,
}

fn main() {
    use std::fmt::Write;

    let listener = TcpListener::bind("127.0.0.1:1237").unwrap();
    let scripts = Mutex::new(HashMap::<String, Script>::new());

    let mut config = Config::new();
    config.wasm_gc(true);
    config.wasm_reference_types(true);
    config.wasm_function_references(true);

    let engine = Engine::new(&config).unwrap();

    fastcgi::run_tcp(
        move |mut req| {
            let Ok(mut scripts) = scripts.lock() else {
                write!(&mut req.stdout(), "Status: 500 Internal Server Error\n\n").unwrap_or(());
                return;
            };

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
                let mut child = Command::new("./cyth")
                    .arg(&path)
                    .arg(path.clone() + ".wasm")
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();

                let status = child.wait().unwrap();

                let mut output = String::new();
                if let Some(stderr) = &mut child.stderr {
                    stderr.read_to_string(&mut output).unwrap();

                    if output.len() > 0 {
                        write!(
                            &mut req.stdout(),
                            "Status: 500 Internal Server Error\n\n{}",
                            output
                        )
                        .unwrap_or(());
                        return;
                    }
                }

                let mut store = Store::new(&engine, String::new());
                let print = Func::wrap(&mut store, |mut caller: Caller<'_, String>, poop: i32| {
                    write!(caller.data_mut(), "{:?}", poop).unwrap();
                });

                let module = Module::from_file(&engine, path.clone() + ".wasm").unwrap();
                let instance = Instance::new(&mut store, &module, &[print.into()]).unwrap();
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

                println!("Compiling and running {} {}", path, status);

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
        },
        &listener,
    );
}
