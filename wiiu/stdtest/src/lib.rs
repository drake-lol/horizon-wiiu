use std::collections::HashMap;

#[no_mangle]
pub extern "C" fn stdtest() -> u32 {
    let mut m = HashMap::new();
    m.insert(1u32, vec![1u8; 4]);
    m.len() as u32 + String::from("x").len() as u32
}
