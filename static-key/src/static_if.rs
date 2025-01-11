/// Read the value of a boolean static key.
///
/// Typically, the value is immediately used by an if.
///
/// # Syntax
///
/// ```
/// # #![feature(asm_goto)]
/// # use static_key::*;
/// static_key!(FOO: bool = true);
///
/// if static_if!(FOO) {
///    /* ... */
/// } else {
///    /* ... */
/// }
///
/// // You may additional specify the likeliness of the branch being taken.
/// if static_if!(FOO, likely) {
///    /* ... */
/// } else {
///    /* ... */
/// }
///
/// if static_if!(FOO, unlikely) {
///    /* ... */
/// } else {
///    /* ... */
/// }
/// ```
///
/// If the likeliness is not explicitly specified, it's considered as unlikely.
#[macro_export]
macro_rules! static_if {
    ($name: path) => {
        $crate::static_if!($name, unlikely)
    };

    ($name: path, likely) => {
        $crate::static_match! { $name: bool;
            #[likely]
            true => true,
            false => false,
        }
    };

    ($name: path, unlikely) => {
        $crate::static_match! { $name: bool;
            true => true,
            #[likely]
            false => false,
        }
    };
}
