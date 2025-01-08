#![feature(asm_goto)]

use std::time::Duration;

use static_key::{static_if, static_key};

static_key!(KEY: bool = false);

pub fn test() {
    loop {
        while !static_if!(KEY) {}
        println!("Static key switches to true!");
        while static_if!(KEY, unlikely) {}
        println!("Static key switches to false!");
    }
}

pub fn update() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        println!("= true");
        KEY.set(true);
        std::thread::sleep(Duration::from_secs(1));
        println!("= false");
        KEY.set(false);
    }
}

fn main() {
    std::thread::spawn(|| {
        test();
    });

    update();
}
