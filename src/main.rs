#![feature(try_blocks)]

mod handler;

use std::{env, path::PathBuf};

use handler::HandlerWrapper;
use serenity::Client;

#[tokio::main]
async fn main() {
    let token = env::var("TATERBOARD_TOKEN").expect("expected token at env `TATERBOARD_TOKEN`");
    let path_to_save = env::args()
        .nth(1)
        .expect("must provide path to  directory to save json");
    let path_to_save = PathBuf::from(path_to_save);

    #[cfg(debug_assertions)]
    let path_to_save = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path_to_save);

    let mut client = Client::builder(&token)
        .event_handler(HandlerWrapper::new(path_to_save))
        .await
        .expect("error creating client");

    let res = client.start().await;
    if let Err(oh_no) = res {
        eprintln!("Client error: {}", oh_no);
    }
}
