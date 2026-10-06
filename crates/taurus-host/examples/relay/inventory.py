"""A stock list for a small shop. See SPEC.md."""


def parse_line(line):
    raise NotImplementedError


def load(text):
    raise NotImplementedError


def total_value(items):
    raise NotImplementedError


def low_stock(items, threshold):
    raise NotImplementedError


def merge(a, b):
    raise NotImplementedError


def apply_sale(items, sku, qty):
    raise NotImplementedError


def restock_plan(items, target, batch=10):
    raise NotImplementedError


def format_report(items):
    raise NotImplementedError
