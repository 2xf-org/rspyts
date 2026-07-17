//! The consumer uses the owner's models directly; there is no bridge DTO.

use cross_package_owner::{Item, NativeCounter};
use rspyts::Type;
use serde::{Deserialize, Serialize};

/// Consumer-owned result containing the canonical item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub item: Item,
    pub included: bool,
}

/// Select a canonical item directly.
#[rspyts::export(python)]
pub fn select(item: Item, rank: i32) -> Selection {
    Selection {
        item,
        included: rank >= 0,
    }
}

/// Python-only behavior may consume a foreign bigint model without exporting it statically.
#[rspyts::export(python)]
pub fn counter_is_nonzero(counter: NativeCounter) -> bool {
    counter.value != 0
}

rspyts::module!(native);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_the_owner_type_without_a_mirror() {
        let item = Item {
            id: "item-3".into(),
            quantity: cross_package_owner::Quantity {
                numerator: 12,
                denominator: 1,
            },
            kind: Some(cross_package_owner::ItemKind::Standard),
            tag: [1, 2, 3, 4],
        };
        let result = select(item.clone(), 1);
        assert_eq!(result.item, item);
        assert!(result.included);
    }
}
