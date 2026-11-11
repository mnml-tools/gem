"""Prints the text of the nested session's X11 clipboard (used to read what Gem copied)."""
import gi

gi.require_version("Gdk", "3.0")
gi.require_version("Gtk", "3.0")
from gi.repository import Gdk, Gtk  # noqa: E402

text = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text()
print(text or "", end="")
