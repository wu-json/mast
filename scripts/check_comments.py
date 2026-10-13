#!/usr/bin/env python3

import ast
import io
import subprocess
import sys
import tokenize


def is_ident(ch):
    return ch == "_" or ch.isalnum()


def rust_spans(text):
    spans = []
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if ch == "/" and nxt == "/":
            end = text.find("\n", i)
            end = n if end == -1 else end
            spans.append((i, end))
            i = end
        elif ch == "/" and nxt == "*":
            start = i
            depth = 1
            i += 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth += 1
                    i += 2
                elif text.startswith("*/", i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
            spans.append((start, i))
        elif ch == '"':
            i = skip_string(text, i + 1)
        elif ch == "'":
            if nxt == "\\":
                i = text.find("'", i + 3) + 1
            elif i + 2 < n and text[i + 2] == "'":
                i += 3
            else:
                i += 1
        elif is_ident(ch):
            start = i
            while i < n and is_ident(text[i]):
                i += 1
            word = text[start:i]
            if word in ("r", "br", "cr") and i < n and text[i] in '#"':
                j = i
                while j < n and text[j] == "#":
                    j += 1
                if j < n and text[j] == '"':
                    close = '"' + "#" * (j - i)
                    i = text.find(close, j + 1) + len(close)
            elif word in ("b", "c") and i < n and text[i] == '"':
                i = skip_string(text, i + 1)
            elif word == "b" and i < n and text[i] == "'":
                i = text.find("'", i + 2 if text[i + 1] == "\\" else i + 1) + 1
        else:
            i += 1
    return spans


def skip_string(text, i):
    while i < len(text):
        if text[i] == "\\":
            i += 2
        elif text[i] == '"':
            return i + 1
        else:
            i += 1
    return i


def python_spans(text):
    lines = text.splitlines(keepends=True)
    offsets = [0]
    for line in lines:
        offsets.append(offsets[-1] + len(line))
    spans = []
    for tok in tokenize.generate_tokens(io.StringIO(text).readline):
        if tok.type != tokenize.COMMENT:
            continue
        (row, col), (erow, ecol) = tok.start, tok.end
        if row == 1 and col == 0 and tok.string.startswith("#!"):
            continue
        spans.append((offsets[row - 1] + col, offsets[erow - 1] + ecol))
    for node in ast.walk(ast.parse(text)):
        if not isinstance(
            node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)
        ):
            continue
        body = node.body
        if (
            body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
            and isinstance(body[0].value.value, str)
        ):
            doc = body[0]
            start = offsets[doc.lineno - 1]
            end = offsets[doc.end_lineno] - 1
            if len(body) == 1:
                indent = lines[doc.lineno - 1][: len(lines[doc.lineno - 1]) - len(lines[doc.lineno - 1].lstrip())]
                spans.append((start, end, indent + "pass"))
            else:
                spans.append((start + len(lines[doc.lineno - 1]) - len(lines[doc.lineno - 1].lstrip()), end))
    return sorted(spans)


def strip(text, spans):
    for span in reversed(spans):
        start, end = span[0], span[1]
        if len(span) == 3:
            text = text[:start] + span[2] + text[end:]
            continue
        line_start = text.rfind("\n", 0, start) + 1
        line_end = text.find("\n", end)
        line_end = len(text) if line_end == -1 else line_end
        before = text[line_start:start]
        after = text[end:line_end]
        if not before.strip() and not after.strip():
            text = text[:line_start] + text[line_end + 1 :]
        elif not after.strip():
            text = text[:line_start] + before.rstrip() + text[line_end:]
        elif not before.strip():
            text = text[:line_start] + before + after.lstrip() + text[line_end:]
        else:
            text = text[:line_start] + before.rstrip() + " " + after.lstrip() + text[line_end:]
    return text


def tracked_files():
    out = subprocess.run(
        ["git", "ls-files", "*.rs", "*.py"], capture_output=True, text=True, check=True
    )
    return out.stdout.split()


def main():
    fix = "--fix" in sys.argv[1:]
    found = False
    for path in tracked_files():
        with open(path, encoding="utf-8") as f:
            text = f.read()
        spans = python_spans(text) if path.endswith(".py") else rust_spans(text)
        if not spans:
            continue
        found = True
        if fix:
            with open(path, "w", encoding="utf-8") as f:
                f.write(strip(text, spans))
            print(f"stripped {len(spans)} comment(s) from {path}")
        else:
            for span in spans:
                line = text.count("\n", 0, span[0]) + 1
                print(f"{path}:{line}: {text[span[0]:span[1]].splitlines()[0].strip()}")
    if found and not fix:
        print("\ncomments are not allowed; run scripts/check_comments.py --fix", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
