# inventory.py

A stock list for a small shop. Every function is pure: it never changes the
list or the items it is given, and returns new ones instead.

An **item** is a dict with exactly four keys:

| key           | type | meaning                                  |
|---------------|------|------------------------------------------|
| `sku`         | str  | stock code, upper case, e.g. `"AB-100"`  |
| `name`        | str  | display name                             |
| `qty`         | int  | units on hand, never negative            |
| `price_cents` | int  | unit price in cents                      |

## parse_line(line)

Parses one line of the form `SKU|Name|qty|price`, where price is written in
dollars with exactly two decimals (`12.50`). Whitespace around each field is
stripped, and the SKU is upper-cased. Raises `ValueError` when the line does
not have exactly four fields, when qty is not a non-negative integer, or when
price is not of the form digits, a dot, two digits.

## load(text)

Parses a whole file. Blank lines and lines whose first non-space character is
`#` are skipped. Returns the items in file order. A bad line raises
`ValueError` whose message starts with `line N: ` (N counts from 1, over every
line of the text including skipped ones).

## total_value(items)

The sum of `qty * price_cents` over every item, in cents.

## low_stock(items, threshold)

The SKUs of items whose qty is strictly below `threshold`, sorted
alphabetically.

## merge(a, b)

Combines two lists by SKU. An SKU in both gets the summed qty, and the name
and price from `b`. The result keeps the order of `a`, followed by the items
only in `b` in their order in `b`.

## apply_sale(items, sku, qty)

Returns the list with `qty` units of `sku` taken off. Raises `KeyError` for
an unknown SKU and `ValueError` when there are not enough units, or when qty
is not positive.

## restock_plan(items, target, batch=10)

For each item whose qty is below `target`, how many units to order to reach
at least `target`, rounded up to a whole number of batches. Returns a dict of
SKU to units, only for items that need ordering.

## format_report(items)

A text table, one line per item in the given order, then a total line:

```
SKU     NAME                   QTY      VALUE
AB-100  Widget                   3      37.50
TOTAL                            3      37.50
```

Columns: SKU left-aligned in 8 characters, name left-aligned in 20 and
truncated to 20, a space, qty right-aligned in 5, value (qty times price, in
dollars with two decimals) right-aligned in 11. The header uses the same
widths. The total line puts `TOTAL` in the SKU and name columns (28
characters, left-aligned), then the summed qty and the summed value. Lines
are joined with `\n` and there is no trailing newline.
