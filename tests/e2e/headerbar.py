"""Clicks the header bar editor's buttons as a screen reader would, for headerbar.sh, and checks
what the settings say after each one. Exits non-zero on the first that doesn't do its job."""
import glob, json, sys, time, gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi

cfg = sys.argv[1]


def find_app():
    desk = Atspi.get_desktop(0)
    for i in range(desk.get_child_count()):
        a = desk.get_child_at_index(i)
        if a and 'tinta' in (a.get_name() or '').lower():
            return a


def find(o, name):
    try:
        if o.get_name() == name:
            return o
        for i in range(o.get_child_count()):
            r = find(o.get_child_at_index(i), name)
            if r:
                return r
    except Exception:
        return None


def click(name):
    for _ in range(50):
        app = find_app()
        b = app and find(app, name)
        if b:
            Atspi.Action.do_action(b, 0)
            time.sleep(0.8)
            return
        time.sleep(0.2)
    print(f'no button "{name}"')
    sys.exit(1)


def bar():
    path = glob.glob(f'{cfg}/**/settings.json', recursive=True)
    if not path:
        return None
    s = json.load(open(path[0]))
    return s.get('headerStart'), s.get('headerEnd')


def expect(step, start, end):
    for _ in range(25):
        if bar() == (start, end):
            print(f'ok: {step}')
            return
        time.sleep(0.2)
    print(f'{step}: wanted {(start, end)}, got {bar()}')
    sys.exit(1)


click('Remove: Settings')
expect('remove', ['view.sidebar'], ['run.file', 'view.split', 'view.terminal'])
click('Move to the Left Side: Run File')
expect('across', ['view.sidebar', 'run.file'], ['view.split', 'view.terminal'])
click('Move Left: Run File')
expect('arrows', ['run.file', 'view.sidebar'], ['view.split', 'view.terminal'])
click('Move Right: Split Editor')
expect('arrows on the right', ['run.file', 'view.sidebar'], ['view.terminal', 'view.split'])
