extern crate wiiu_atomics;

fn main() {
    println!("nettest: start");
    let url = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
    match ureq::get(url).call() {
        Ok(resp) => {
            println!("nettest: HTTP {}", resp.status());
            let mut body = String::new();
            use std::io::Read;
            let n = resp.into_reader().take(4096).read_to_string(&mut body).unwrap_or(0);
            println!("nettest: {n} bytes: {}", body.chars().take(120).collect::<String>());
        }
        Err(e) => println!("nettest: error: {e}"),
    }
}
