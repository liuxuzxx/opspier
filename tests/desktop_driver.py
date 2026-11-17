"""Small WebDriver client for the isolated desktop integration suite."""
import base64,json,socket,time,urllib.error,urllib.request
ELEMENT = "element-6066-11e4-a52e-4f735466cecf"

def free_port():
    with socket.socket() as stream:
        stream.bind(("127.0.0.1", 0))
        return stream.getsockname()[1]


class Driver:
    def __init__(self, port):
        self.base = f"http://127.0.0.1:{port}"
        self.sid = ""

    def call(self, method, path, data=None):
        request = urllib.request.Request(self.base + path, data=None if data is None else json.dumps(data).encode(), method=method, headers={"Content-Type":"application/json"})
        try:
            with urllib.request.urlopen(request, timeout=45) as response:
                result = json.load(response)
        except urllib.error.HTTPError as error:
            # Driver errors can contain page inputs; report only the error type.
            result = json.load(error)
            raise RuntimeError("WebDriver " + str(result.get("value", {}).get("error", error.code))) from None
        value = result.get("value")
        if isinstance(value, dict) and value.get("error"):
            raise RuntimeError("WebDriver " + value["error"])
        return value

    def cmd(self, method, path, data=None):
        return self.call(method, "/session/" + self.sid + path, data)

    def js(self, script, *args):
        return self.cmd("POST", "/execute/sync", {"script":script, "args":list(args)})

    def wait(self, script, *args, timeout=12):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = self.js(script, *args)
            if result:
                return result
            time.sleep(.1)
        raise AssertionError("UI condition timed out: " + script[:110])

    def element(self, selector):
        return self.wait("return [...document.querySelectorAll(arguments[0])].find(e=>e.getClientRects().length && !e.closest('[hidden]'))", selector)

    def click(self, element):
        self.js("arguments[0].scrollIntoView({block:'nearest'});arguments[0].click()",element)

    def button(self, text):
        self.click(self.wait("return [...document.querySelectorAll('button,[role=menuitem],[role=tab]')].find(e=>e.textContent.replace(/\\s/g,'')===arguments[0].replace(/\\s/g,'') && e.getClientRects().length && !e.closest('[hidden]') && !e.disabled)", text))

    def fill(self, selector, text):
        element = self.element(selector)
        # Native setter plus input event exercises the real controlled form handler.
        self.js("const e=arguments[0];const p=e.tagName==='TEXTAREA'?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(p,'value').set.call(e,arguments[1]);e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));",element,str(text))

    def select(self, selector, text):
        element=self.wait("return [...document.querySelectorAll(arguments[0])].find(e=>e.getClientRects().length&&!e.closest('[hidden]')&&!(e.closest('.ant-select')||e).classList.contains('ant-select-disabled'))",selector)
        self.js("const e=arguments[0];const target=e.querySelector('input')||e;target.dispatchEvent(new MouseEvent('mousedown',{bubbles:true,button:0}));target.click()",element)
        self.click(self.wait("return [...document.querySelectorAll('.ant-select-item-option')].find(e=>e.textContent.trim()===arguments[0]&&e.getClientRects().length)", text))

    def text(self, text):
        return self.wait("return document.body.innerText.includes(arguments[0])", text)

    def ipc(self, command, args=None):
        result = self.cmd("POST", "/execute/async", {"script":"const done=arguments[arguments.length-1];window.__TAURI_INTERNALS__.invoke(arguments[0],arguments[1]).then(value=>done({ok:true,value}),()=>done({ok:false}));", "args":[command, args or {}]})
        assert result["ok"], "real IPC failed: " + command
        return result.get("value")

    def screenshot(self, path):
        path.write_bytes(base64.b64decode(self.cmd("GET", "/screenshot")))



def request_native_close(window_id):
    """只向隔离显示器中的测试窗口发送原生关闭协议，触发真实应用退出事件。"""
    import ctypes
    import os
    assert os.environ.get('DISPLAY') == ':196'

    class ClientData(ctypes.Union):
        """X11客户端消息中的固定大小数据区。"""
        _fields_ = [('bytes', ctypes.c_char * 20), ('shorts', ctypes.c_short * 10), ('longs', ctypes.c_long * 5)]

    class ClientMessage(ctypes.Structure):
        """原生窗口管理关闭事件的布局。"""
        _fields_ = [('type', ctypes.c_int), ('serial', ctypes.c_ulong), ('send_event', ctypes.c_int),
                    ('display', ctypes.c_void_p), ('window', ctypes.c_ulong), ('message_type', ctypes.c_ulong),
                    ('format', ctypes.c_int), ('data', ClientData)]

    class Event(ctypes.Union):
        """保留XEvent规定的完整内存大小，供XSendEvent读取。"""
        _fields_ = [('client', ClientMessage), ('padding', ctypes.c_long * 24)]

    x11 = ctypes.CDLL('libX11.so.6')
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.POINTER(Event)]
    x11.XFlush.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    display = x11.XOpenDisplay(None)
    assert display, 'isolated X11 display unavailable'
    try:
        event = Event()
        event.client.type = 33
        event.client.display = display
        event.client.window = int(window_id)
        event.client.message_type = x11.XInternAtom(display, b'WM_PROTOCOLS', 0)
        event.client.format = 32
        event.client.data.longs[0] = x11.XInternAtom(display, b'WM_DELETE_WINDOW', 0)
        assert x11.XSendEvent(display, int(window_id), 0, 0, ctypes.byref(event)) != 0
        x11.XFlush(display)
    finally:
        x11.XCloseDisplay(display)
