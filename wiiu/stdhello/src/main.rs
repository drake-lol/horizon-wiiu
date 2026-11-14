extern crate wiiu_atomics;

fn main() {
    println!("stdhello: std runtime started");
    let v: Vec<u32> = (0..1000).collect();
    println!("sum = {}", v.iter().sum::<u32>());
    let t = std::thread::spawn(|| 40 + 2);
    println!("thread = {:?}", t.join());
}
