import unittest


class SampleTest(unittest.TestCase):
    def test_pass(self):
        self.assertEqual(1 + 1, 2)

    def test_fail(self):
        self.assertEqual(1 + 1, 3)


if __name__ == "__main__":
    unittest.main()
