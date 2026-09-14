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
    goal_gui_launcher = root / "goal-gui.sh"
    goal_gui_launcher.chmod(goal_gui_launcher.stat().st_mode | 0o111)
    for name, terminal, exe, args, comment, icon in (
        ("Launch Roundtable", False, launcher, "",
         "Let your connected AIs discuss a topic", "system-users"),
        ("Roundtable Terminal", True, launcher, " --terminal",
         "Let your connected AIs discuss a topic", "system-users"),
        ("Roundtable Goal Mode", False, goal_gui_launcher, "",
         "Set a bounded, sandboxed, independently-reviewed goal for a connected seat",
         "system-run"),
    ):
        path = root / (name + ".desktop")
        path.write_text(
            "[Desktop Entry]\nType=Application\n"
            f"Name={name}\n"
            f"Comment={comment}\n"
            f"Exec={quote_exec(str(exe))}{args}\n"
            f"Path={root}\nTerminal={str(terminal).lower()}\n"
            f"Icon={icon}\nCategories=Utility;\n",
            encoding="utf-8",
        )
        path.chmod(0o755)
        print(path)


if __name__ == "__main__":
    main()
