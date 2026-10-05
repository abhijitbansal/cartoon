"""Human-readable durations: "1h30m" <-> seconds."""

import re

_UNITS = {"d": 86400, "h": 3600, "m": 60, "s": 1}
_PART = re.compile(r"(\d+)([dhms])")


def parse(text: str) -> int:
    """Parse "1d2h3m4s" (any subset, in order) into seconds."""
    text = text.strip().lower()
    if not text:
        raise ValueError("empty duration")
    pos = 0
    total = 0
    for match in _PART.finditer(text):
        if match.start() != pos:
            raise ValueError(f"bad duration: {text!r}")
        total += int(match.group(1)) * _UNITS[match.group(2)]
        pos = match.end()
    if pos != len(text):
        raise ValueError(f"bad duration: {text!r}")
    return total


def format(seconds: int) -> str:  # noqa: A001 - mirrors parse()
    """Format seconds as the shortest "1d2h3m4s" form; 0 is "0s"."""
    if seconds < 0:
        raise ValueError("negative duration")
    if seconds == 0:
        return "0s"
    out = []
    for unit, size in _UNITS.items():
        count, seconds = divmod(seconds, size)
        if count:
            out.append(f"{count}{unit}")
    return "".join(out)
