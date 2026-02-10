# CythCGI
A server-side scripting environment for the [Cyth](https://github.com/vldr/Cyth) programming language, written in Rust. 

It enables a lightweight, PHP-like development experience, allowing Cyth code to be embedded directly into HTML for dynamic content generation.

```html
<!DOCTYPE html>
<html>
    <head>
        <title>Example</title>
    </head>
    <body>
        <?
            print("Hi, I'm a Cyth script!")
        ?>
    </body>
</html>
```

## Building

To build CythCGI, you will need to have [Rust](https://rust-lang.org/) installed. 

Run the following commands from the root directory (in a terminal):
```bash
cargo build --release
```

The output executable will be located in the `target/release` folder.