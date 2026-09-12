#!/usr/bin/env python3
"""Generate optional Linux launchers for this checkout, without installing them."""
from pathlib import Path


def quote_exec(value: str) -> str:
    value = value.replace("%", "%%")
    for character in ('\\', '"', '`', '$'):
        value = value.replace(character, '\\' + character)
    return '"' + value + '"'


def main() -> None:
    root = Path(__file__).resolve().parent
    launcher = root / "roundtable.sh"
    launcher.chmod(launcher.stat().st_mode | 0o111)
    for name, terminal, args in (
        ("Launch Roundtable", False, ""),
        ("Roundtable Terminal", True, " --terminal"),
    ):
        path = root / (name + ".desktop")
        path.write_text(
            "[Desktop Entry]\nType=Application\n"
            f"Name={name}\n"
            "Comment=Let your connected AIs discuss a topic\n"
            f"Exec={quote_exec(str(launcher))}{args}\n"
            f"Path={root}\nTerminal={str(terminal).lower()}\n"
            "Icon=system-users\nCategories=Utility;\n",
            encoding="utf-8",
        )
        path.chmod(0o755)
        print(path)


if __name__ == "__main__":
    main()
