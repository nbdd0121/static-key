use std::sync::{Mutex, MutexGuard, Once};

use rustix::mm::MprotectFlags;

fn sync_core() {
    rustix::process::membarrier(rustix::process::MembarrierCommand::PrivateExpeditedSyncCore)
        .unwrap();
}

/// A guard indicating ability to modify text.
///
/// Note that possession of this guard still permits other threads to execute text,
/// so writer still needs to be careful.
pub struct TextGuard {
    need_sync: bool,
    _mutex_guard: MutexGuard<'static, ()>,
}

impl TextGuard {
    pub unsafe fn skip_sync(mut self) {
        self.need_sync = false;
    }
}

impl Drop for TextGuard {
    fn drop(&mut self) {
        if self.need_sync {
            sync_core();
        }
    }
}

static TEXT_LOCK: Mutex<()> = Mutex::new(());

/// Obtain a lock to be able to modify text.
pub fn lock_text() -> TextGuard {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        rustix::process::membarrier(
            rustix::process::MembarrierCommand::RegisterPrivateExpeditedSyncCore,
        )
        .unwrap();
    });

    TextGuard {
        need_sync: false,
        _mutex_guard: TEXT_LOCK.lock().unwrap(),
    }
}

/// Registry code to be eligible for modification.
pub unsafe fn register_code(_: &mut TextGuard, insn: *mut u8, bytes: usize) {
    let addr = insn.addr();
    let page_size = rustix::param::page_size();
    let page_down = addr & !(page_size - 1);
    let page_up = (addr + (bytes - 1)) & !(page_size - 1);
    let page_len = page_up - page_down + page_size;

    unsafe {
        rustix::mm::mprotect(
            page_down as _,
            page_len,
            MprotectFlags::READ | MprotectFlags::WRITE | MprotectFlags::EXEC,
        )
        .unwrap();
    }
}

/// Modify code starting from `address` to `new_code`.
///
/// # Safety
///
/// This is a very dangerous operation, and a lot of things need to uphold to ensure soundness.
///
/// * The `address` must point to a call-site that is designed for runtime replacement.
/// * The `address` must be previous registered to be modifiable.
/// * `new_code` must an instruction that still make the replaced machine code function correctly.
///
/// Importantly, to ensure correctness, only a single instruction may be replaced. (Or, when the first
/// instruction can never fallthrough, and the following instructions can never be otherwise executed,
/// it's also permissible to replace the whole sequence). This is to prevent the case where a core is
/// executing half-way through a sequence, and replacing causes the execution to start half-way through.
pub unsafe fn replace_instruction(text_guard: &mut TextGuard, insn: *mut u8, new_insn: &[u8]) {
    // If there's no updates needed, then just skip. This can avoid having to perform core-syncing.
    if unsafe { core::slice::from_raw_parts(insn, new_insn.len()) } == new_insn {
        return;
    }

    text_guard.need_sync = true;

    unsafe { arch::replace_instruction(insn, new_insn) };
}

#[cfg(target_arch = "x86_64")]
mod arch {
    use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

    pub(super) unsafe fn replace_instruction(insn: *mut u8, new_insn: &[u8]) {
        // Simple path for single-byte replacement.
        if new_insn.len() == 1 {
            unsafe { (*insn.cast::<AtomicU8>()).store(new_insn[0], Ordering::SeqCst) };
            return;
        }

        // For multi-byte replacement, we must ensure that first 2 bytes can be replaced atomically.
        let address = insn.addr();
        if address % 8 == 7 {
            // The address spans 2 QWORD boundary.
            // Technically if both bytes are still in the same cache line, we can still replace
            // atomically, but this usually means that the instruction generator failed to ensure
            // correct padding, so it's better to abort early than trying hard.
            panic!("Atomicity of instruction replacement cannot be guaranteed");
        }

        unsafe fn replace_within_qword(insn: *mut u8, new_insn: &[u8]) {
            unsafe {
                let qword = &*insn.map_addr(|x| x & !7).cast::<AtomicU64>();
                let mut bytes = qword.load(Ordering::Relaxed).to_ne_bytes();
                bytes[insn.addr() % 8..][..new_insn.len()].copy_from_slice(new_insn);
                // No need to use cmpxchg here since we require the text lock to be held.
                qword.store(u64::from_ne_bytes(bytes), Ordering::SeqCst);
            }
        }

        // The instruction to be replaced fits in a QWORD.
        // We can do an atomic replacement.
        if address % 8 + new_insn.len() <= 8 {
            unsafe { replace_within_qword(insn, new_insn) };
            return;
        }

        // This will span multiple QWORD boundary.
        // When this happens we use alternative replacement strategy.

        // First, replace the first instruction with a short backward-jump so it loops forever.
        // EB FE: jmp $-2
        unsafe { replace_within_qword(insn, &[0xEB, 0xFE]) };

        // Now, we can replace the rest of bytes.
        for i in 2..new_insn.len() {
            unsafe { (*insn.add(i).cast::<AtomicU8>()).store(new_insn[i], Ordering::SeqCst) };
        }

        // Finally, replace the initial jump with target.
        unsafe { replace_within_qword(insn, &new_insn[..2]) };
    }
}

#[cfg(target_arch = "riscv64")]
mod arch {
    use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};

    pub(super) unsafe fn replace_instruction(insn: *mut u8, new_insn: &[u8]) {
        assert_eq!(insn.addr() % 2, 0);
        assert!(new_insn.len() == 2 || new_insn.len() == 4);

        // Simple path for half-word replacement.
        if new_insn.len() == 2 {
            (*insn.cast::<AtomicU16>()).store(
                u16::from_ne_bytes(new_insn.try_into().unwrap()),
                Ordering::Relaxed,
            );
            return;
        }

        // Simple path for aligned word replacement.
        if insn.addr() % 4 == 0 {
            (*insn.cast::<AtomicU32>()).store(
                u32::from_ne_bytes(new_insn.try_into().unwrap()),
                Ordering::Relaxed,
            );
            return;
        }

        // If an instruction is not aligned, then compressed instructions must be enabled.
        // In such case, we replace first half-word with a `c.j 0`.
        (*insn.cast::<AtomicU16>()).store(0xA001, Ordering::Relaxed);
        super::sync_core();

        (*insn.add(2).cast::<AtomicU16>()).store(
            u16::from_ne_bytes(new_insn[2..].try_into().unwrap()),
            Ordering::Relaxed,
        );
        super::sync_core();

        (*insn.cast::<AtomicU16>()).store(
            u16::from_ne_bytes(new_insn[..2].try_into().unwrap()),
            Ordering::Relaxed,
        );
        super::sync_core();
    }
}
