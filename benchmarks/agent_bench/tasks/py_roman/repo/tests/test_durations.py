import pytest

from roman import durations

CASES = [
    ("0s", 0),
    ("1s", 1),
    ("59s", 59),
    ("1m", 60),
    ("1m1s", 61),
    ("1h", 3600),
    ("1h30m", 5400),
    ("2h5s", 7205),
    ("1d", 86400),
    ("1d1s", 86401),
    ("3d4h5m6s", 3 * 86400 + 4 * 3600 + 5 * 60 + 6),
]


@pytest.mark.parametrize("text,seconds", CASES)
def test_parse(text, seconds):
    assert durations.parse(text) == seconds


@pytest.mark.parametrize("text,seconds", CASES)
def test_format(text, seconds):
    assert durations.format(seconds) == text


@pytest.mark.parametrize("bad", ["", "1x", "h1", "1h 2m", "1.5h"])
def test_parse_rejects(bad):
    with pytest.raises(ValueError):
        durations.parse(bad)
