"""Reads the editor as a screen reader would, for a11y.sh. Exits non-zero on what it'd miss."""
import sys, time, gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi, GLib
def find_app():
    desk = Atspi.get_desktop(0)
    for i in range(desk.get_child_count()):
        a = desk.get_child_at_index(i)
        if a and 'tinta' in (a.get_name() or '').lower():
            return a
def find_text(o):
    try:
        if o.get_role() == Atspi.Role.TEXT or o.get_role_name() in ('text', 'entry'):
            if o.get_text_iface() and o.get_name():
                return o
    except Exception:
        return None
    for i in range(o.get_child_count()):
        r = find_text(o.get_child_at_index(i))
        if r: return r
for _ in range(50):
    app = find_app()
    ed = app and find_text(app)
    if ed: break
    time.sleep(0.2)
if not ed:
    print("no text object: a screen reader sees no text"); sys.exit(1)
t = ed.get_text_iface()
n = t.get_character_count()
print(f'role={ed.get_role_name()} name="{ed.get_name()}" chars={n} caret={t.get_caret_offset()}')
assert ed.get_role() == Atspi.Role.TEXT and ed.get_name() == 'a.rs' and n == 28
print('text:', repr(Atspi.Text.get_text(ed, 0, n)))
line = Atspi.Text.get_string_at_offset(ed, 0, Atspi.TextGranularity.LINE)
print('line at 0:', repr(line.content), line.start_offset, line.end_offset)
ext = Atspi.Text.get_character_extents(ed, 3, Atspi.CoordType.WINDOW)
print('extents of char 3:', ext.x, ext.y, ext.width, ext.height)
seen, pairs = [], []
def on(ev):
    if ev.source.get_name() == ed.get_name():
        seen.append(ev.type)
        pairs.append((ev.type, ev.any_data if isinstance(ev.any_data, str) else None))
        print('EVENT', ev.type, ev.detail1, ev.detail2, repr(ev.any_data) if isinstance(ev.any_data, str) else '')
l = Atspi.EventListener.new(on)
for e in ('object:text-caret-moved', 'object:text-changed:insert', 'object:text-changed:delete'):
    l.register(e)
loop = GLib.MainLoop()
GLib.timeout_add(7000, loop.quit)
loop.run()
print('final caret', Atspi.Text.get_caret_offset(ed), 'text:', repr(Atspi.Text.get_text(ed, 0, Atspi.Text.get_character_count(ed))))
assert seen.count('object:text-caret-moved') >= 3, seen
assert ('object:text-changed:insert', 'x') in pairs and ('object:text-changed:delete', 'x') in pairs, pairs
