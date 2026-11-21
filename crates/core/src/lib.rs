pub mod journal;
pub mod plan;
pub mod sanitize;
pub mod template;

pub use journal::{JournalEntry, Operation};
pub use plan::{NameAllocator, Plan, PlanEntry, PlanStatus, RelatedMove};
pub use sanitize::{sanitize, sanitize_or_renamed, sanitize_relative_dir};
pub use template::{FieldValues, FormatOptions, RelatedMode, SanitizeMode, Template};

pub const BUILTIN_DJI: &str = include_str!("../../../templates/dji.toml");
pub const BUILTIN_INSTA360: &str = include_str!("../../../templates/insta360.toml");
pub const BUILTIN_GOPRO: &str = include_str!("../../../templates/gopro.toml");

pub fn builtin_template(name: &str) -> Option<&'static str> {
    match name {
        "dji" => Some(BUILTIN_DJI),
        "insta360" => Some(BUILTIN_INSTA360),
        "gopro" => Some(BUILTIN_GOPRO),
        _ => None,
    }
}
