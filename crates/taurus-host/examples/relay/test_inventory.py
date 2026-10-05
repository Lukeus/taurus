import unittest

from inventory import (apply_sale, format_report, load, low_stock, merge,
                       parse_line, restock_plan, total_value)


def item(sku, name, qty, price):
    return {"sku": sku, "name": name, "qty": qty, "price_cents": price}


class ParseLine(unittest.TestCase):
    def test_fields(self):
        self.assertEqual(parse_line("AB-100|Widget|3|12.50"), item("AB-100", "Widget", 3, 1250))

    def test_whitespace_and_case(self):
        self.assertEqual(parse_line("  ab-1 | Big Bolt | 0 | 0.05 "), item("AB-1", "Big Bolt", 0, 5))

    def test_wrong_field_count(self):
        with self.assertRaises(ValueError):
            parse_line("AB|Widget|3")

    def test_negative_qty(self):
        with self.assertRaises(ValueError):
            parse_line("AB|Widget|-1|1.00")

    def test_bad_price(self):
        for price in ["1.5", "1", "1.505", "a.bc", "-1.00"]:
            with self.assertRaises(ValueError, msg=price):
                parse_line(f"AB|Widget|1|{price}")


class Load(unittest.TestCase):
    def test_skips_blank_and_comments(self):
        text = "# stock\n\nAB|A|1|1.00\n   # note\nCD|C|2|2.00\n"
        self.assertEqual([i["sku"] for i in load(text)], ["AB", "CD"])

    def test_error_names_the_line(self):
        text = "# header\nAB|A|1|1.00\nbroken\n"
        with self.assertRaises(ValueError) as caught:
            load(text)
        self.assertTrue(str(caught.exception).startswith("line 3: "), str(caught.exception))

    def test_empty(self):
        self.assertEqual(load(""), [])


class TotalValue(unittest.TestCase):
    def test_sum(self):
        self.assertEqual(total_value([item("A", "a", 2, 150), item("B", "b", 3, 1000)]), 3300)

    def test_empty(self):
        self.assertEqual(total_value([]), 0)


class LowStock(unittest.TestCase):
    def test_strictly_below_and_sorted(self):
        items = [item("ZZ", "z", 1, 1), item("AA", "a", 5, 1), item("MM", "m", 4, 1)]
        self.assertEqual(low_stock(items, 5), ["MM", "ZZ"])

    def test_none(self):
        self.assertEqual(low_stock([item("A", "a", 9, 1)], 5), [])


class Merge(unittest.TestCase):
    def test_sums_and_takes_b(self):
        a = [item("A", "old", 1, 100), item("B", "b", 2, 200)]
        b = [item("C", "c", 5, 500), item("A", "new", 4, 150)]
        self.assertEqual(merge(a, b), [
            item("A", "new", 5, 150),
            item("B", "b", 2, 200),
            item("C", "c", 5, 500),
        ])

    def test_does_not_mutate(self):
        a = [item("A", "a", 1, 100)]
        b = [item("A", "a", 1, 100)]
        merge(a, b)
        self.assertEqual(a, [item("A", "a", 1, 100)])
        self.assertEqual(b, [item("A", "a", 1, 100)])


class ApplySale(unittest.TestCase):
    def test_takes_units(self):
        items = [item("A", "a", 5, 100), item("B", "b", 1, 100)]
        self.assertEqual(apply_sale(items, "A", 2), [item("A", "a", 3, 100), item("B", "b", 1, 100)])
        self.assertEqual(items[0]["qty"], 5)

    def test_unknown(self):
        with self.assertRaises(KeyError):
            apply_sale([item("A", "a", 5, 100)], "Z", 1)

    def test_not_enough(self):
        with self.assertRaises(ValueError):
            apply_sale([item("A", "a", 1, 100)], "A", 2)

    def test_not_positive(self):
        with self.assertRaises(ValueError):
            apply_sale([item("A", "a", 1, 100)], "A", 0)


class RestockPlan(unittest.TestCase):
    def test_batches(self):
        items = [item("A", "a", 3, 1), item("B", "b", 20, 1), item("C", "c", 0, 1)]
        self.assertEqual(restock_plan(items, 20), {"A": 20, "C": 20})

    def test_custom_batch(self):
        self.assertEqual(restock_plan([item("A", "a", 7, 1)], 10, batch=4), {"A": 4})

    def test_rounds_up(self):
        self.assertEqual(restock_plan([item("A", "a", 9, 1)], 10, batch=6), {"A": 6})


class FormatReport(unittest.TestCase):
    def test_one_item(self):
        expected = (
            "SKU     NAME                   QTY      VALUE\n"
            "AB-100  Widget                   3      37.50\n"
            "TOTAL                            3      37.50"
        )
        self.assertEqual(format_report([item("AB-100", "Widget", 3, 1250)]), expected)

    def test_truncates_long_names(self):
        report = format_report([item("A", "An extremely long product name", 1, 100)])
        self.assertIn("An extremely long pr ", report.splitlines()[1])

    def test_total_sums(self):
        report = format_report([item("A", "a", 2, 100), item("B", "b", 3, 1000)])
        self.assertEqual(report.splitlines()[-1], "TOTAL                            5      32.00")

    def test_empty(self):
        self.assertEqual(
            format_report([]),
            "SKU     NAME                   QTY      VALUE\n"
            "TOTAL                            0       0.00",
        )


if __name__ == "__main__":
    unittest.main()
