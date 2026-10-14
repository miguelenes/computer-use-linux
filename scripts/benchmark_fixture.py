#!/usr/bin/python3
"""Launch a small, reversible GTK fixture for computer-use benchmarks.

The fixture has three independent actions. Each changes one accessible status
label and Reset returns it to Ready. It performs no file, network, or shell
operations and is safe to launch in both the native desktop and the VM.
"""

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gio, GLib, Gtk  # noqa: E402

GLib.set_prgname("computer-use-linux-benchmark")
GLib.set_application_name("computer-use-linux-benchmark")


class BenchmarkFixture(Gtk.Application):
    def __init__(self) -> None:
        super().__init__(
            application_id="dev.computeruselinux.BenchmarkFixture",
            flags=Gio.ApplicationFlags.NON_UNIQUE,
        )

    def do_activate(self) -> None:
        window = Gtk.ApplicationWindow(application=self)
        window.set_title("Computer Use Benchmark Fixture")
        window.set_default_size(420, 220)

        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        content.set_border_width(16)
        window.add(content)

        status = Gtk.Label(label="Ready")
        status.set_name("Benchmark status")
        content.pack_start(status, False, False, 0)

        for action in ("Alpha", "Beta", "Gamma"):
            button = Gtk.Button(label=f"Enable {action}")
            button.connect(
                "clicked", lambda _button, value=action: status.set_text(f"{value} enabled")
            )
            content.pack_start(button, False, False, 0)

        reset = Gtk.Button(label="Reset")
        reset.connect("clicked", lambda _button: status.set_text("Ready"))
        content.pack_start(reset, False, False, 0)
        window.show_all()


if __name__ == "__main__":
    BenchmarkFixture().run(None)
