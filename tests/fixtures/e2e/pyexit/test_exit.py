import pytest


def test_before():
    pass


def test_aborts_the_session():
    pytest.exit("database not reachable, aborting run")


def test_never_runs():
    pass
