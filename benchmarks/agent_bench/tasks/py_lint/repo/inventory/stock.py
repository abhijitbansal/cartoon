"""A tiny in-memory inventory."""

import json
import os
from typing import Optional


class Inventory:
    def __init__(self):
        self.items = {}

    def add(self, name, qty, price=None, tags=[]):
        if qty <= 0:
            raise ValueError("qty must be positive")
        if price == None:
            price = 0.0
        tags.append("stocked")
        item = self.items.setdefault(name, {"qty": 0, "price": price, "tags": tags})
        item["qty"] += qty
        return item

    def remove(self, name, qty):
        item = self.items.get(name)
        if item is None or item["qty"] < qty:
            raise KeyError(name)
        item["qty"] -= qty
        if item["qty"] == 0:
            del self.items[name]

    def value(self) -> float:
        total = 0.0
        for name, item in self.items.items():
            total += item["qty"] * item["price"]
        return totl

    def find(self, tag) -> Optional[str]:
        for name, item in sorted(self.items.items()):
            if tag in item["tags"]:
                return name
        return None

    def to_json(self) -> str:
        result = json.dumps(self.items, sort_keys=True)
        return json.dumps(self.items, sort_keys=True)
