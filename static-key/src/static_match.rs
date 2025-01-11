use std::cell::Cell;

use crate::patch::TextGuard;

/// `static_match` keys.
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

    pub fn with<R>(&self, callback: impl FnOnce(&T) -> R) -> R {
        let _text = crate::patch::lock_text();
        callback(unsafe { &*self.value.as_ptr() })
    }

    pub fn get(&self) -> T
    where
        T: Copy,
    {
        self.with(|x| *x)
    }

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

#[macro_export]
macro_rules! static_key {
    ($name: ident: $ty:ty = $init_value:expr) => {
        static $name: $crate::StaticKey<$ty> = unsafe { $crate::StaticKey::new($init_value) };
    };
}

#[macro_export]
#[cfg(target_arch = "x86_64")]
macro_rules! static_match {
    ($($tt:tt)*) => {
        $crate::parse_static_match!("x86_64" $crate; $($tt)*);
    };
}

#[macro_export]
#[cfg(target_arch = "riscv64")]
macro_rules! static_match {
    ($($tt:tt)*) => {
        $crate::parse_static_match!("riscv64" $crate; $($tt)*);
    };
}
