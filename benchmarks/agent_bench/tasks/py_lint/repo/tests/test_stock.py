import json

import pytest

from inventory.stock import Inventory


def test_add_and_value():
    inv = Inventory()
    inv.add("apple", 3, price=0.5)
    inv.add("pear", 2, price=1.25)
    assert inv.value() == pytest.approx(4.0)


def test_default_price_is_zero():
    inv = Inventory()
    inv.add("rock", 10)
    assert inv.value() == 0.0


def test_add_rejects_non_positive():
    with pytest.raises(ValueError):
        Inventory().add("x", 0)


def test_remove():
    inv = Inventory()
    inv.add("apple", 3, price=1.0)
    inv.remove("apple", 3)
    assert inv.items == {}
    with pytest.raises(KeyError):
        inv.remove("apple", 1)


def test_tags_are_not_shared_between_items():
    inv = Inventory()
    inv.add("a", 1, tags=["fruit"])
    inv.add("b", 1)
    inv.add("c", 1)
    assert inv.items["b"]["tags"] == ["stocked"]
    assert inv.items["c"]["tags"] == ["stocked"]
    assert inv.items["a"]["tags"] == ["fruit", "stocked"]


def test_caller_tags_not_mutated():
    mine = ["fruit"]
    Inventory().add("a", 1, tags=mine)
    assert mine == ["fruit"]


def test_find():
    inv = Inventory()
    inv.add("b", 1, tags=["x"])
    inv.add("a", 1, tags=["x"])
    assert inv.find("x") == "a"
    assert inv.find("nope") is None


def test_to_json():
    inv = Inventory()
    inv.add("a", 2, price=1.5)
    assert json.loads(inv.to_json()) == {"a": {"qty": 2, "price": 1.5, "tags": ["stocked"]}}
