//! Stage the built web client into `OUT_DIR` so `src/api/webui.rs` can embed it
//! with `include_str!`. If `web/dist` has not been built yet (no `npm run
//! build`), a small placeholder is written instead so the crate still compiles.

use std::path::Path;
use std::{env, fs};

const ASSETS: &[&str] = &["index.html", "nwr-stations.json", "fm-stations.json"];

const PLACEHOLDER_HTML: &str = "<!doctype html><meta charset=utf-8><title>LANline</title>\
<body style=\"font:16px system-ui;margin:3rem;max-width:34rem\">\
<h1>LANline</h1><p>The web client bundle was not built into this server. Run:</p>\
<pre>npm --prefix web install &amp;&amp; npm --prefix web run build</pre>\
<p>then rebuild the server.</p>";

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = env::var("OUT_DIR").unwrap();
    let dist = Path::new(&manifest).join("../web/dist");
    let staged = Path::new(&out_dir).join("webui");
    fs::create_dir_all(&staged).unwrap();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", dist.display());

    for name in ASSETS {
        let src = dist.join(name);
        println!("cargo:rerun-if-changed={}", src.display());
        let bytes = fs::read(&src).unwrap_or_else(|_| {
            if *name == "index.html" {
                PLACEHOLDER_HTML.as_bytes().to_vec()
            } else {
                b"[]".to_vec()
            }
        });
        fs::write(staged.join(name), bytes).unwrap();
    }
}
