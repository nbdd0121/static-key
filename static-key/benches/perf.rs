#![feature(asm_goto)]
#![feature(test)]

extern crate test;

use core::sync::atomic::*;
use std::hint::black_box;
use static_key::{static_if, static_key};

#[inline(never)]
fn foo() {
    black_box(());
}

#[inline(never)]
fn bar() {
    black_box(1);
}

#[bench]
fn bench_static_if(b: &mut test::Bencher) {
    static_key!(KEY: bool = false);
    KEY.set(false);
    b.iter(|| {
        if static_if!(KEY) {
            foo();
        } else {
            bar();
        }
    });
}

#[bench]
fn bench_atomic_bool(b: &mut test::Bencher) {
    static KEY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    KEY.store(test::black_box(false), Ordering::Relaxed);
    b.iter(|| {
        if KEY.load(Ordering::Relaxed) {
            foo();
        } else {
            bar();
        }
    });
}

#[bench]
fn bench_static_if_flush(b: &mut test::Bencher) {
    static_key!(KEY: bool = false);
    KEY.set(false);
    b.iter(|| {
        if static_if!(KEY) {
            foo();
        } else {
            bar();
        }

        unsafe { core::arch::x86_64::_mm_clflush(&raw const KEY as _) };
    });
}

#[bench]
fn bench_atomic_bool_flush(b: &mut test::Bencher) {
    static KEY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

    KEY.store(test::black_box(false), Ordering::Relaxed);
    b.iter(|| {
        if KEY.load(Ordering::Relaxed) {
            foo();
        } else {
            bar();
        }

        unsafe { core::arch::x86_64::_mm_clflush(&raw const KEY as _) };
    });
}

#[bench]
fn update_key(b: &mut test::Bencher) {
    static_key!(KEY: bool = false);

    // Create a use-site otherwise benchmark doesn't mean anything.
    if black_box(false) {
        static_if!(KEY);
    }

    b.iter(|| {
        KEY.set(!KEY.get());
    });
}
