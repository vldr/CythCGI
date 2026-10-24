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

## Installation
The following installation is for `nginx`, some of the steps listed may work for other webservers.

1. [Build](#building) or [download](https://github.com/vldr/CythCGI/releases/latest) a precompiled binary of CythCGI.
2. Place the CythCGI binary somewhere, for this installation, it will be placed in `/path/to/cyth-cgi`.
* Make sure you enable executable permissions for this binary, for instance, `chmod +x cyth-cgi`.
* Make sure your web server's user can access and run the executable, for this installation, the `nginx` user will be running this executable.

3. Create a `cyth.service` file and place it inside `/etc/systemd/system/` with the following contents:
```ini
[Unit]
Description=cyth
After=network.target

[Service]
Type=simple
User=nginx
Group=nginx
ExecStart=/path/to/cyth-cgi /run/cyth/cyth.sock
RuntimeDirectory=cyth
RuntimeDirectoryMode=0755
Restart=always
RestartSec=5s
LimitNOFILE=infinity
StandardError=journal
StandardOutput=journal

[Install]
WantedBy=multi-user.target
```
- Change `/path/to/cyth-cgi` to your executable's path.
- Update `User=nginx`, `Group=nginx` to whichever user your web server uses.

4. Run the following commands to enable the `cyth-cgi` service:
```bash
sudo systemctl daemon-reload
sudo systemctl enable cyth.service
```

5. Run the following commands to start the `cyth-cgi` service:

```bash
sudo systemctl start cyth.service
```

6. Verify that the `cyth-cgi` service is running:
```bash
sudo systemctl status cyth.service
```

* You may need to repeat step 6, as starting these services up can be finicky.

7. In your `nginx.conf`, inside a `server` clause. Add the following code to make `nginx` detect `.cy` files as Cyth script code:

```
location ~ \.cy$ {
    include fastcgi_params;
            fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;
            fastcgi_param QUERY_STRING    $query_string;
    fastcgi_pass unix:/run/cyth/cyth.sock;
    fastcgi_index index.cy;
    fastcgi_keep_conn off;
}
```