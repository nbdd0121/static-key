use std::cell::Cell;

use crate::patch::TextGuard;

/// An object that is rarely modified but frequently matched on.
///
/// This type can only be constructed using [`static_key!`] macro.
///
/// [`static_key!`]: `crate::static_key!`
pub struct StaticKey<T: 'static> {
    /// Call/use sites that reference this key.
    callsites: Cell<Option<&'static CallSite<T>>>,
    value: Cell<T>,
}

unsafe impl<T: Send> Send for StaticKey<T> {}
unsafe impl<T: Send> Sync for StaticKey<T> {}

impl<T> core::fmt::Debug for StaticKey<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticKey").finish()
    }
}

/// Individual call sites of a specific static key.
#[doc(hidden)]
#[repr(C)]
pub struct CallSite<T: 'static> {
    key: &'static StaticKey<T>,
    /// Address of the callsite.
    address: *mut u8,
    matcher: fn(&T) -> usize,
    next: Cell<Option<&'static CallSite<T>>>,
    labels: [u32; 0],
}

unsafe impl<T: Send> Send for CallSite<T> {}
unsafe impl<T: Send> Sync for CallSite<T> {}

impl<T> CallSite<T> {
    #[cfg(target_arch = "x86_64")]
    /// Update the callsite
    fn update(&self, text: &mut TextGuard, value: &T) {
        let callee = (self.matcher)(&value);

        let mut patch;
        if callee == usize::MAX {
            // 5-byte nop.
            patch = [0x0f, 0x1f, 0x44, 0x00, 0x00];
        } else {
            let offset = unsafe { *self.labels.as_ptr().add(callee) };
            patch = [0xe9, 0x00, 0x00, 0x00, 0x00];
            patch[1..].copy_from_slice(&offset.to_le_bytes());
        }

        unsafe {
            crate::patch::replace_instruction(text, self.address, &patch);
        }
    }

    #[cfg(target_arch = "riscv64")]
    /// Update the callsite
    fn update(&self, text: &mut TextGuard, value: &T) {
        let callee = (self.matcher)(&value);

        let patch;
        if callee == usize::MAX {
            // uncompressed nop.
            patch = 0x00000013;
        } else {
            let offset = unsafe { *self.labels.as_ptr().add(callee) };
            patch = (offset >> 20 & 1) << 31
                | (offset >> 1 & 0x3FF) << 21
                | (offset >> 11 & 1) << 20
                | (offset >> 12 & 0xFF) << 12
                | 0x6F;
        }

        unsafe {
            crate::patch::replace_instruction(text, self.address, &patch.to_ne_bytes());
        }
    }

    /// Register a callsite. Must be called before potential execution of the binary.
    pub unsafe extern "C" fn register(&'static self) {
        let mut text = crate::patch::lock_text();

        #[cfg(target_arch = "x86_64")]
        let insn_len = 5;

        #[cfg(target_arch = "riscv64")]
        let insn_len = 4;

        // Register the address to be patched to be eligible for modification.
        unsafe { crate::patch::register_code(&mut text, self.address, insn_len) };

        // Insert the callsite into the key linked list.
        self.next.set(self.key.callsites.get());
        self.key.callsites.set(Some(self));
        self.update(&mut text, unsafe { &*self.key.value.as_ptr() });

        // Registration happens at binary loading time and normal execution hasn't started.
        // So syncing is not necessary here.
        unsafe { text.skip_sync() };
    }
}

impl<T> StaticKey<T> {
    #[doc(hidden)]
    #[inline]
    pub const unsafe fn new(state: T) -> Self {
        Self {
            value: Cell::new(state),
            callsites: Cell::new(None),
        }
    }

    /// Acquire a reference to the value of this key.
    ///
    /// # Performance
    ///
    /// `StaticKey`s are not intended to be used like cell/locks, so this is not an especially
    /// optimised accessor. This method should be rarely used.
    pub fn with<R>(&self, callback: impl FnOnce(&T) -> R) -> R {
        let _text = crate::patch::lock_text();
        callback(unsafe { &*self.value.as_ptr() })
    }

    /// Returns a copy of the value of this key.
    ///
    /// # Performance
    ///
    /// `StaticKey`s are not intended to be used like cell/locks, so this is not an especially
    /// optimised accessor. This method should be rarely used.
    pub fn get(&self) -> T
    where
        T: Copy,
    {
        self.with(|x| *x)
    }

    /// Sets the value of this key.
    ///
    /// # Performance
    ///
    /// `StaticKey`s are not intended to be frequently updated, so this is slow in performance.
    /// This method should be rarely used.
    pub fn set(&self, value: T) {
        let mut text = crate::patch::lock_text();
        let mut callsite = self.callsites.get();
        while let Some(site) = callsite {
            site.update(&mut text, &value);
            callsite = site.next.get();
        }
        self.value.set(value);
    }
}

/// Declares a new static key.
///
/// # Syntax
///
/// ```
/// # use static_key::*;
/// static_key!(pub FOO: u32 = 1);
/// static_key!(BAR: bool = false);
/// ```
///
/// The key must be `Send`:
/// ```compile_fail
/// # use static_key::*;
/// static_key!(BAZ: *const () = core::ptr::null()); // ERROR
/// ```
///
/// and the initializer must be const:
/// ```compile_fail
/// # use static_key::*;
/// fn baz() -> u32 { 1 }
/// static_key!(BAZ: u32 = baz()); // ERROR
/// ```
#[macro_export]
macro_rules! static_key {
    ($vis:vis $name: ident: $ty:ty = $init_value:expr) => {
        $vis static $name: $crate::StaticKey<$ty> = {
            let value: $ty = $init_value;
            unsafe { $crate::StaticKey::new(value) }
        };
    };
}

#[doc(hidden)]
#[macro_export]
#[cfg(target_arch = "x86_64")]
macro_rules! with_arch {
    ($callback: ident, $($tt:tt)*) => {
        $crate::$callback!("x86_64", $($tt)*)
    };
}

#[doc(hidden)]
#[macro_export]
#[cfg(target_arch = "riscv64")]
macro_rules! with_arch {
    ($callback: path, $($tt:tt)*) => {
        $crate::$callback!("riscv64", $($tt)*)
    };
}

/// Match on a static key.
///
/// # Syntax
///
/// This macro uses a syntax similar to `match`:
/// ```
/// # #![feature(asm_goto)]
/// # use static_key::*;
/// static_key!(FOO: u32 = 1);
///
/// let value = static_match! {
///   // First declare what's the key and its type
///   FOO: u32;
///   // Then match arm follows. Note that the type being matched is a reference type.
///   1 => 1,
///   x if *x == 2 => {
///     println!("Guard is supported");
///     2
///   }
///   // You can put `#[likely]` on an arm to optimise for common case.
///   #[likely]
///   0 => 0,
///   _ => unreachable!(),
/// };
/// ```
///
/// The entire `static_match!` will be compiled to either a single instruction, either
/// a no-op or a single jump.
///
/// Note that the match arm may bind variables for the guard only; bindings are not visible
/// inside the match arm body.
#[macro_export]
macro_rules! static_match {
    ($($tt:tt)*) => {
        $crate::with_arch!(parse_static_match, $crate; $($tt)*);
    };
}
