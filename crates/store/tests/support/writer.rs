//! JSON-lines worker used only by the mixed-process compatibility fixture.

use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    path::Path,
};
use tmux_workspaces_store::{Document, Store};

fn emit(status: &str, base: &Document, local: &tmux_workspaces_core::Layout) {
    let state = base.with_model(local, Path::new("layouts.db")).unwrap();
    println!(
        "{}",
        json!({
            "status": status,
            "state": serde_json::from_str::<Value>(state.raw_json()).unwrap(),
            "base": serde_json::from_str::<Value>(base.raw_json()).unwrap(),
        })
    );
    io::stdout().flush().unwrap();
}

fn main() {
    let directory = std::env::args_os()
        .nth(1)
        .expect("synthetic library directory");
    let mut store = Store::open_existing_writable(Path::new(&directory)).unwrap();
    let mut base = store.load().unwrap();
    let mut local = base.model().clone();
    emit("loaded", &base, &local);
    for line in io::stdin().lock().lines() {
        let command: Value = serde_json::from_str(&line.unwrap()).unwrap();
        match command["op"].as_str().unwrap() {
            "quit" => break,
            "state" => {
                local = Document::parse(command["state"].to_string(), Path::new("layouts.db"))
                    .unwrap()
                    .model()
                    .clone();
                emit("edited", &base, &local);
            }
            "workspace_name" => {
                let workspace = local
                    .workspaces
                    .iter_mut()
                    .find(|item| item.id == command["id"].as_str().unwrap())
                    .unwrap();
                workspace.name = command["name"].as_str().unwrap().into();
                emit("edited", &base, &local);
            }
            "tab_name" => {
                let tab = local
                    .workspaces
                    .iter_mut()
                    .flat_map(|space| &mut space.tabs)
                    .find(|item| item.id == command["id"].as_str().unwrap())
                    .unwrap();
                tab.name = command["name"].as_str().unwrap().into();
                emit("edited", &base, &local);
            }
            "save" => {
                let result = store.save(&mut base, &local).unwrap();
                local = result.document.model().clone();
                emit(
                    if result.conflict { "conflict" } else { "saved" },
                    &base,
                    &local,
                );
            }
            _ => panic!("unknown fixture operation"),
        }
    }
}
