from __future__ import annotations

from example.consumer import authored_label
from example.consumer.contracts import Item as ConsumerItem
from example.consumer.contracts import counter_is_nonzero, select
from example.owner.contracts import Item as OwnerItem
from example.owner.contracts import ItemKind, Quantity, native_counter


def test_consumer_reuses_the_owner_model() -> None:
    assert authored_label() == "consumer-authored"
    assert ConsumerItem is OwnerItem

    item = OwnerItem(
        id="item-3",
        quantity=Quantity(numerator=12, denominator=1),
        kind=ItemKind.Standard,
        tag=b"\x01\x02\x03\x04",
    )
    result = select(item, 1)

    assert result.item is item or result.item == item
    assert isinstance(result.item, OwnerItem)
    assert counter_is_nonzero(native_counter(1))
