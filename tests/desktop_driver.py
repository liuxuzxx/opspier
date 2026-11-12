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

