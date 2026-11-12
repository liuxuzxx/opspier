"""A private-display native GTK drag source; stdin contains only an explicit file path."""
import json
import sys
import gi
gi.require_version('Gtk','3.0')
from gi.repository import Gtk,Gdk
from pathlib import Path
path=Path(json.loads(sys.stdin.readline())['path'])
window=Gtk.Window(title='OPS fixture file drag')
window.set_default_size(170,110);window.move(0,0);window.set_keep_above(True)
label=Gtk.Button(label='Drag kubeconfig')
label.drag_source_set(Gdk.ModifierType.BUTTON1_MASK,[Gtk.TargetEntry.new('text/uri-list',0,0)],Gdk.DragAction.COPY)
def give_data(widget,context,selection,info,timestamp):
    selection.set_uris([path.as_uri()]);print('data',flush=True)
label.connect('drag-data-get',give_data)
label.connect('drag-begin',lambda *args:print('begin',flush=True))
label.connect('drag-end',lambda *args:print('end',flush=True))
label.connect('drag-failed',lambda *args:print('failed',flush=True))
window.add(label);window.connect('destroy',Gtk.main_quit);window.show_all()
print('ready',flush=True)
Gtk.main()
