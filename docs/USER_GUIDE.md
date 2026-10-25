# User guide — building searches

Every search is a tree of **rules** inside **groups**. Once you know the three building blocks,
any mix of AND/OR at any depth is just clicks.

## Building blocks

| Piece | What it does | In the UI |
|---|---|---|
| **Rule** | One condition on one column: column · operator · value(s) | **+ Rule** |
| **Group** | A pair of brackets. Its **AND / OR** toggle decides how its children combine | **+ Group** (inside any group, any depth) |
| **NOT** | Flips a whole group's result | **NOT** checkbox on a group |

Two shortcuts:

- **Several values in one rule are OR'ed.** `country` equals `USA` ⏎ `India` ⏎ means "USA **or** India".
- **Quick search is always AND'ed** with the filters and looks in every column.

Method: write the search with brackets first. Every pair of brackets is a **+ Group**; the word
between its items (AND/OR) is that group's toggle.

## Operators

| Operator | Matches | Notes |
|---|---|---|
| contains / starts with / ends with | substring | case-insensitive unless **Aa** is on |
| equals (exact) / not equals | whole value | not equals keeps NULL cells |
| pattern ( * ? ) | wildcard | `*` any run of characters, `?` exactly one |
| matches regex / not matches regex | regular expression | PostgreSQL: POSIX (`~*`); BigQuery: RE2 |
| > ≥ < ≤ | comparison in the column's real type | numbers, dates (`2023-01-01`), timestamps |
| is NULL / is not NULL | missing values | no value needed |
| is empty / blank | NULL, empty string, `{}` or `[]` | no value needed |

A range needs **two rules in an AND group** (`age ≥ 30` and `age ≤ 50`); two values in one rule
would mean `≥ 30 OR ≥ 50`.

## Examples (row counts for the bundled mock data)

**Simple** — `country contains india` → 120 rows.

**Range + any-of** — `(country = USA or India) AND age 30–50` → 80 rows:

```
[AND]
 ├─ country · equals · USA ⏎ India ⏎
 ├─ age · ≥ · 30
 └─ age · ≤ · 50
```

**Mixed AND/OR, two levels** → 163 rows:

```
[AND]
 ├─ active · equals · true
 └─ [OR]
     ├─ [AND]
     │   ├─ email · matches regex · ^(alice|bob)\d+@
     │   └─ balance · > · 5000
     ├─ [AND]
     │   ├─ tags · contains · vip
     │   └─ country · not equals · USA
     └─ notes · is NULL
```

**NOT and three levels** → 23 rows:

```
[AND]
 ├─ [OR]
 │   ├─ notes · pattern · Order #10*DHL
 │   └─ notes · starts with · #ERROR
 └─ [OR] + ☑ NOT
     ├─ profile · contains · enterprise
     └─ [AND]
         ├─ signup_at · ≥ · 2022-01-01
         └─ signup_at · < · 2023-01-01
```

## Tips

- **Show SQL** prints the exact `WHERE` clause — the quickest way to check your brackets.
- Drag any rule or group by its **⠿** handle to reorder it or move it into another group.
- Empty rules and groups are ignored, so build the structure first and fill in values later.
- The address bar always holds the current search; **🔗 Save & share** gives a short `/s/<id>` link.
- **⬇ Excel / ⬇ JSON** export every matching row, not just the visible page.
- An invalid regex or a value that doesn't fit the column type shows the database's error message
  instead of results; fix the value and the table comes back.
