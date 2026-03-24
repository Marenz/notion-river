use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

fn socket_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("notion-river.sock")
}

fn usage() {
    eprintln!(
        "usage:\n  notion-ctl list-windows\n  notion-ctl list-workspaces\n  notion-ctl subscribe-workspaces\n  notion-ctl subscribe-workspace <name>\n  notion-ctl subscribe-output <output-name>\n  notion-ctl focus-window <id>\n  notion-ctl switch-workspace <name>\n  notion-ctl bind <app_id> <workspace> <frame_index> [WxH]\n  notion-ctl unbind <app_id>\n  notion-ctl set-fixed-dimensions <app_id> <WxH|clear>"
    );
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        usage();
        std::process::exit(2);
    }

    let is_subscribe = args[0] == "subscribe-workspaces"
        || args[0] == "subscribe-workspace"
        || args[0] == "subscribe-output";

    let cmd = match args[0].as_str() {
        "list-windows" => "list-windows".to_string(),
        "list-workspaces" => "list-workspaces".to_string(),
        "subscribe-workspaces" => "subscribe-workspaces".to_string(),
        "subscribe-workspace" => {
            if args.len() < 2 {
                usage();
                std::process::exit(2);
            }
            format!("subscribe-workspace {}", args[1..].join(" "))
        }
        "subscribe-output" => {
            if args.len() < 2 {
                usage();
                std::process::exit(2);
            }
            format!("subscribe-output {}", args[1..].join(" "))
        }
        "focus-window" => {
            if args.len() != 2 {
                usage();
                std::process::exit(2);
            }
            format!("focus-window {}", args[1])
        }
        "switch-workspace" => {
            if args.len() < 2 {
                usage();
                std::process::exit(2);
            }
            format!("switch-workspace {}", args[1..].join(" "))
        }
        "set-fixed-dimensions" => {
            if args.len() != 3 {
                usage();
                std::process::exit(2);
            }
            format!("set-fixed-dimensions {} {}", args[1], args[2])
        }
        "bind" => {
            if args.len() < 4 {
                usage();
                std::process::exit(2);
            }
            let mut cmd = format!("bind {} {} {}", args[1], args[2], args[3]);
            if args.len() >= 5 {
                cmd.push_str(&format!(" {}", args[4]));
            }
            cmd
        }
        "unbind" => {
            if args.len() != 2 {
                usage();
                std::process::exit(2);
            }
            format!("unbind {}", args[1])
        }
        _ => {
            usage();
            std::process::exit(2);
        }
    };

    let path = socket_path();
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to connect to {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    // Send command with newline (required for subscribe, backwards compatible for others).
    if let Err(e) = stream.write_all(format!("{cmd}\n").as_bytes()) {
        eprintln!("failed to write command: {e}");
        std::process::exit(1);
    }

    if is_subscribe {
        // Streaming mode: read and print lines as they arrive.
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            match line {
                Ok(line) => println!("{line}"),
                Err(_) => break,
            }
        }
    } else {
        // Legacy one-shot mode.
        let _ = stream.shutdown(std::net::Shutdown::Write);
        let mut response = String::new();
        match stream.read_to_string(&mut response) {
            Ok(_) => {
                print!("{response}");
            }
            Err(e) => {
                eprintln!("failed to read response: {e}");
                std::process::exit(1);
            }
        }
    }
}
