#[macro_export]
macro_rules! static_if {
    ($name: path) => {
        $crate::static_if!($name, unlikely)
    };

    ($name: path, likely) => {
        $crate::static_match!{ $name: bool;
            #[likely]
            true => true,
            false => false,
        }
    };

    ($name: path, unlikely) => {
        $crate::static_match!{ $name: bool;
            true => true,
            #[likely]
            false => false,
        }
    };
}
