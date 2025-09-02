use hyper::service::{make_service_fn, service_fn};
use clap::Parser;
use serde::Deserialize;
use hyper::{Body, Request, Response, Server};
use std::convert::Infallible;
use std::net::SocketAddr;
use tokio::signal;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// 配置文件路径，默认 /etc/app/application.toml
    #[arg(long, default_value = "application.toml")]
    conf: String,
}

#[derive(Deserialize)]
struct Settings {
    #[serde(default = "default_port")]
    port: u16,
}
fn default_port() -> u16 { 8080 }

#[tokio::main]
async fn main() {
    // 解析命令行参数
    let cli = Cli::parse();

    // 读取并解析配置文件（可选）
    let settings: Settings = match std::fs::read_to_string(&cli.conf) {
        Ok(content) => match toml::from_str(&content) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("❌ 配置文件解析错误: {}", e);
                std::process::exit(1);
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!("未发现自定义配置 {}, 使用默认配置", &cli.conf);
            Settings { port: default_port() }
        }
        Err(err) => {
            eprintln!("⚠️  无法读取配置文件 {}: {}，使用默认配置", &cli.conf, err);
            Settings { port: default_port() }
        }
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], settings.port));

    let make_svc = make_service_fn(|_conn| async {
        Ok::<_, Infallible>(service_fn(|_req: Request<Body>| async {
            Ok::<_, Infallible>(Response::new(Body::from("Hello World\n")))
        }))
    });

    let server = Server::bind(&addr).serve(make_svc);

    println!("Config: {} | Open http://{}/", &cli.conf, addr);

    // 支持优雅关停：收到 Ctrl+C 或 SIGTERM (k8s 下的 "kill -TERM") 后退出
    let graceful = server.with_graceful_shutdown(shutdown_signal());

    if let Err(e) = graceful.await {
        eprintln!("server error: {}", e);
    }
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    // 同时监听 Ctrl+C(SIGINT) 与 SIGTERM
    let mut term_stream = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    tokio::select! {
        _ = signal::ctrl_c() => {},   // Ctrl+C
        _ = term_stream.recv() => {}, // Kubernetes 等发送的 SIGTERM
    }
    println!("Shutting down...");
}
