import unittest


class ManyTests(unittest.TestCase):
    def test_ok_a(self):
        self.assertTrue(True)

    def test_ok_b(self):
        self.assertEqual("x" * 3, "xxx")

    def test_ok_c(self):
        self.assertIn(2, [1, 2, 3])

    def test_ok_d(self):
        self.assertIsNone(None)

    def _explode(self, name):
        payload = {"user": name, "roles": ["admin", "editor", "viewer"], "active": True}
        self.assertEqual(payload["roles"], ["admin", "editor"], f"unexpected roles for {name}")

    def test_fail_alpha(self):
        self._explode("alpha")

    def test_fail_beta(self):
        self._explode("beta")

    def test_fail_gamma(self):
        self._explode("gamma")

    def test_fail_delta(self):
        self._explode("delta")

    def test_fail_epsilon(self):
        self._explode("epsilon")

    def test_fail_zeta(self):
        self._explode("zeta")
