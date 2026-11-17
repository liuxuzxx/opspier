"""在私有显示器和配置目录验证真实桌面标题栏，不操作用户窗口或连接。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import traceback

from desktop_driver import Driver, free_port, ELEMENT

ROOT = Path(__file__).resolve().parents[1]
DISPLAY = ':197'
WM_ROOT = Path(os.environ.get('OPSPIER_WM_ROOT', '/'))


def inside(application, output):
    """启动真实候选应用，检查合并高度和原生窗口操作。"""
    processes, logs, owned = [], [], []
    driver = None
    report = {'status': 'running', 'checks': [], 'measurements': {}}

    def launch(args, name):
        """仅记录自有子进程，退出时按所属进程组清理。"""
        stream = (output / name).open('wb')
        logs.append(stream)
        process = subprocess.Popen(args, stdout=stream, stderr=stream, start_new_session=True)
        processes.append(process)
        return process

    def check(name, condition):
        """失败即保存现场，禁止用后续成功覆盖失败结论。"""
        assert condition, name
        report['checks'].append(name)
        print('PASS ' + name, flush=True)

    def native(command):
        """只查询主窗口自身状态，真实点击仍经过界面按钮。"""
        return driver.ipc('plugin:window|' + command, {'label': 'main'})

    def mouse(*args):
        """真实鼠标动作限定在本测试的私有 DISPLAY。"""
        subprocess.run([os.environ['OPSPIER_XDOTOOL'], *map(str, args)], check=True, timeout=12)

    def click(selector):
        """在原生几何与视口稳定后发送真实点击，不依赖驱动的旧窗口坐标。"""
        settle_window()
        point = driver.wait("const e=[...document.querySelectorAll(arguments[0])].find(e=>e.getClientRects().length);if(!e)return false;const r=e.getBoundingClientRect(),x=r.x+r.width/2,y=r.y+r.height/2,hit=document.elementFromPoint(x,y);return e===hit||e.contains(hit)?{x,y,scale:devicePixelRatio}:false", selector)
        position = native('outer_position')
        # 重复点击同一坐标不会产生MotionNotify，不能用mousemove --sync等待该事件。
        mouse('mousemove', position['x'] + round(point['x'] * point['scale']),
              position['y'] + round(point['y'] * point['scale']), 'sleep', '.05',
              'mousedown', '1', 'sleep', '.08', 'mouseup', '1')

    def settle_window():
        """等待原生大小、位置及WebView尺寸连续一致，期间不重放点击。"""
        previous, stable = None, 0
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            current = (native('outer_position'), native('inner_size'), driver.js('return [innerWidth,innerHeight,devicePixelRatio]'))
            size, viewport = current[1], current[2]
            synchronized = abs(size['width'] / viewport[2] - viewport[0]) <= 4 and abs(size['height'] / viewport[2] - viewport[1]) <= 4
            stable = stable + 1 if current == previous and synchronized else 0
            if stable >= 2:
                return
            previous = current
            time.sleep(.1)
        raise AssertionError('native window geometry did not settle')

    try:
        assert not Path('/tmp/.X11-unix/X197').exists() and not Path('/tmp/.X197-lock').exists()
        launch([os.environ['OPSPIER_XVFB'], DISPLAY, '-screen', '0', '1900x1200x24', '-nolisten', 'tcp', '-ac'], 'xvfb.log')
        for _ in range(100):
            if Path('/tmp/.X11-unix/X197').exists():
                break
            time.sleep(.1)
        wm_config = Path(os.environ['HOME']) / 'openbox.xml'
        wm_config.write_text('<?xml version="1.0"?><openbox_config xmlns="http://openbox.org/3.4/rc"><theme><name>Clearlooks</name></theme><focus><focusNew>yes</focusNew></focus></openbox_config>')
        launch([os.environ.get('OPSPIER_OPENBOX', str(WM_ROOT / 'usr/bin/openbox')), '--sm-disable', '--config-file', str(wm_config)], 'wm.log')
        time.sleep(.6)
        port, native_port = free_port(), free_port()
        driver_process = launch([os.environ['OPSPIER_TAURI_DRIVER'], '--port', str(port), '--native-port', str(native_port),
                                 '--native-host', '127.0.0.1', '--native-driver', os.environ['OPSPIER_WEBKIT_DRIVER']], 'driver.log')
        driver = Driver(port)
        for _ in range(100):
            try:
                driver.call('GET', '/status')
                break
            except (OSError, RuntimeError):
                time.sleep(.1)
        session = driver.call('POST', '/session', {'capabilities': {'alwaysMatch': {
            'browserName': 'wry', 'tauri:options': {'application': str(application)}}}})
        driver.sid = session['sessionId']
        driver.cmd('POST', '/timeouts', {'script': 30000, 'implicit': 0, 'pageLoad': 30000})
        driver.text('从左侧新建或打开连接')
        driver.cmd('POST', '/window/rect', {'x': 80, 'y': 70, 'width': 1200, 'height': 820})
        rows = subprocess.check_output(['ps', '-eo', 'pid,pgid,comm'], text=True).splitlines()[1:]
        owned = [int(row.split()[0]) for row in rows if len(row.split()) == 3
                 and int(row.split()[1]) == driver_process.pid and row.split()[2] == 'opspier']
        check('one-owned-application', len(owned) == 1)
        report['applicationSha256'] = hashlib.sha256(application.read_bytes()).hexdigest()
        check('actual-candidate-binary', hashlib.sha256(Path(f'/proc/{owned[0]}/exe').read_bytes()).hexdigest() == report['applicationSha256'])
        driver.js("window.__titlebarErrors=[];window.__titlebarMouse=[];window.addEventListener('error',e=>window.__titlebarErrors.push(e.message));window.addEventListener('unhandledrejection',e=>window.__titlebarErrors.push(String(e.reason)));window.addEventListener('mousedown',e=>window.__titlebarMouse.push({detail:e.detail,x:e.clientX,y:e.clientY,tag:e.target.tagName,drag:e.target.getAttribute('data-tauri-drag-region')}),true);")
        dimensions = driver.js("const h=document.querySelector('.app-header'),r=h.getBoundingClientRect(),c=document.querySelector('.app-content').getBoundingClientRect();return {height:r.height,top:r.top,bottom:r.bottom,contentTop:c.top,background:getComputedStyle(h).backgroundColor,sidebarBackground:getComputedStyle(document.querySelector('.connection-sidebar')).backgroundColor};")
        report['measurements']['header'] = dimensions
        driver.screenshot(output / 'header-initial.png')
        check('single-compact-header-at-most-36px', dimensions['height'] <= 36)
        check('workspace-starts-directly-below-header', dimensions['top'] == 0 and dimensions['contentTop'] == dimensions['bottom'])
        check('matching-titlebar-and-sidebar-background', dimensions['background'] == dimensions['sidebarBackground'])
        check('native-titlebar-disabled', native('is_decorated') is False)
        client_size = native('inner_size')
        viewport = driver.js('return {width:innerWidth,height:innerHeight,scale:devicePixelRatio}')
        report['measurements']['nativeClient'] = {'size': client_size, 'viewport': viewport}
        check('no-extra-native-titlebar-height', abs(client_size['height'] / viewport['scale'] - viewport['height']) <= 4)
        check('brand-and-workbench-share-header', driver.js("return document.querySelector('.app-header').textContent.includes('OpsPier')&&document.querySelector('.app-header').textContent.includes('运维工作台')"))
        version = json.loads((ROOT / 'package.json').read_text())['version']
        check('version-remains-accessible-in-footer', driver.js("return document.querySelector('.app-footer').textContent.includes(arguments[0])", version))
        click('.sidebar-toggle')
        driver.wait("return document.querySelector('.sidebar-toggle').getAttribute('aria-expanded')==='false'")
        check('sidebar-collapses', driver.js("return !document.querySelector('.connection-sidebar').getClientRects().length"))
        click('.sidebar-toggle')
        driver.wait("return document.querySelector('.sidebar-toggle').getAttribute('aria-expanded')==='true'")
        check('sidebar-restores', driver.js("return !!document.querySelector('.connection-sidebar').getClientRects().length"))
        before = native('outer_position')
        drag = driver.js("const r=document.querySelector('[data-testid=titlebar-drag-region]').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}")
        mouse('mousemove', '--sync', before['x'] + int(drag['x']), before['y'] + int(drag['y']), 'mousedown', '1', 'sleep', '.2',
              'mousemove', '--sync', before['x'] + int(drag['x']) + 120, before['y'] + int(drag['y']) + 80, 'sleep', '.2', 'mouseup', '1')
        after = driver.wait("return window.__TAURI_INTERNALS__?true:false")
        after = native('outer_position')
        report['measurements']['drag'] = {'before': before, 'after': after}
        check('native-titlebar-drag-moves-window', after['x'] - before['x'] >= 80 and after['y'] - before['y'] >= 50)
        before_size, position = native('outer_size'), native('outer_position')
        mouse('mousemove', '--sync', position['x'] + before_size['width'] - 2, position['y'] + before_size['height'] - 2,
              'mousedown', '1', 'sleep', '.2', 'mousemove', '--sync', position['x'] + before_size['width'] + 98,
              position['y'] + before_size['height'] + 68, 'sleep', '.2', 'mouseup', '1')
        after_size = native('outer_size')
        report['measurements']['resize'] = {'before': before_size, 'after': after_size}
        check('frameless-window-retains-corner-resize', after_size['width'] - before_size['width'] >= 60 and after_size['height'] - before_size['height'] >= 40)
        driver.select('.connection-sidebar .ant-select', 'SSH')
        driver.click(driver.element('.connection-sidebar button.ant-btn-primary'))
        driver.fill('.ant-modal #name', '布局验证草稿')
        driver.fill('.ant-modal #host', 'layout.invalid')
        click('[aria-label="最大化窗口"]')
        for _ in range(50):
            if native('is_maximized'):
                break
            time.sleep(.1)
        check('maximize-button-controls-real-window', native('is_maximized') is True)
        click('[aria-label="还原窗口"]')
        for _ in range(50):
            if not native('is_maximized'):
                break
            time.sleep(.1)
        check('restore-button-controls-real-window', native('is_maximized') is False)
        check('window-actions-preserve-form-draft', driver.js("return document.querySelector('.ant-modal #host').value==='layout.invalid'&&document.querySelector('.ant-modal #name').value==='布局验证草稿'"))
        driver.click(driver.element('.ant-modal-close'))
        driver.wait("return ![...document.querySelectorAll('.ant-modal')].some(e=>e.getClientRects().length)")
        settle_window()
        position = native('outer_position')
        drag = driver.js("const r=document.querySelector('[data-testid=titlebar-drag-region]').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}")
        # 为异步原生拖动留出正常按键持续时间，避免零时长松开先于窗口管理器抓取。
        mouse('mousemove', '--sync', position['x'] + int(drag['x']), position['y'] + int(drag['y']),
              'mousedown', '1', 'sleep', '.08', 'mouseup', '1', 'sleep', '.08',
              'mousedown', '1', 'sleep', '.08', 'mouseup', '1')
        for _ in range(50):
            if native('is_maximized'):
                break
            time.sleep(.1)
        check('double-click-titlebar-maximizes', native('is_maximized') is True)
        report['measurements']['doubleClickMouse'] = driver.js("return window.__titlebarMouse.filter(e=>e.drag==='true').slice(-2)")
        click('[aria-label="还原窗口"]')
        driver.wait("return !!document.querySelector('[aria-label=最大化窗口]')")
        click('[aria-label="最小化窗口"]')
        for _ in range(50):
            if native('is_minimized'):
                break
            time.sleep(.1)
        check('minimize-button-controls-real-window', native('is_minimized') is True)
        windows = subprocess.check_output([os.environ['OPSPIER_XDOTOOL'], 'search', '--pid', str(owned[0]), '--name', 'OpsPier'], text=True).split()
        # 排除 GTK 的隐藏组窗口，任务栏恢复应作用于真正的工作台顶层窗口。
        main_windows = [window for window in windows if '运维工作台' in subprocess.check_output(
            [os.environ['OPSPIER_XDOTOOL'], 'getwindowname', window], text=True)]
        assert len(main_windows) == 1
        report['measurements']['restoreWindowId'] = main_windows[0]
        mouse('windowactivate', '--sync', main_windows[0])
        for _ in range(50):
            if not native('is_minimized'):
                break
            time.sleep(.1)
        check('window-restores-after-minimize', native('is_minimized') is False)
        driver.cmd('POST', '/window/rect', {'width': 1040, 'height': 720})
        driver.wait("return innerWidth<=1040")
        check('header-controls-fit-minimum-window', driver.js("const h=document.querySelector('.app-header'),b=[...h.querySelectorAll('button')];return h.scrollWidth<=innerWidth&&b.every(e=>{const r=e.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&r.top>=0&&r.bottom<=36})"))
        driver.screenshot(output / 'header-minimum.png')
        check('no-page-errors', driver.js('return window.__titlebarErrors.length') == 0)
        try:
            click('[aria-label="关闭窗口"]')
        except (OSError, RuntimeError):
            if any(Path(f'/proc/{pid}').exists() for pid in owned):
                raise
        for _ in range(100):
            if all(not Path(f'/proc/{pid}').exists() for pid in owned):
                break
            time.sleep(.1)
        check('close-button-exits-owned-application', all(not Path(f'/proc/{pid}').exists() for pid in owned))
        report['status'] = 'passed'
    except Exception as error:
        report['status'] = 'failed'
        report['failure'] = {'type': type(error).__name__, 'message': str(error),
                             'stack': [{'file': Path(x.filename).name, 'line': x.lineno, 'function': x.name} for x in traceback.extract_tb(error.__traceback__)]}
        if driver and driver.sid:
            try:
                report['debug'] = driver.js("return {errors:window.__titlebarErrors,mouse:window.__titlebarMouse?.slice(-12),viewport:{width:innerWidth,height:innerHeight},masks:[...document.querySelectorAll('.ant-modal-mask,.ant-modal-wrap')].map(e=>({visible:!!e.getClientRects().length,zIndex:getComputedStyle(e).zIndex}))}")
                driver.screenshot(output / 'failure.png')
            except (OSError, RuntimeError):
                pass
        print('FAIL ' + type(error).__name__ + ': ' + str(error), flush=True)
    finally:
        if driver and driver.sid:
            if any(Path(f'/proc/{pid}').exists() for pid in owned):
                try:
                    native('close')
                except (OSError, RuntimeError):
                    pass
            try:
                driver.cmd('DELETE', '')
            except (OSError, RuntimeError):
                pass
        for process in reversed(processes):
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        for process in processes:
            try:
                process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
        for stream in logs:
            stream.close()
        deadline = time.monotonic() + 8
        while any(Path(f'/proc/{pid}').exists() for pid in owned) and time.monotonic() < deadline:
            time.sleep(.1)
        report['cleanup'] = {'processesExited': all(p.poll() is not None for p in processes),
                             'ownedApplicationsExited': all(not Path(f'/proc/{pid}').exists() for pid in owned),
                             'privateDisplayRemoved': not Path('/tmp/.X11-unix/X197').exists()}
        (output / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    return 0 if report['status'] == 'passed' and all(report['cleanup'].values()) else 1


def main():
    """每轮创建独立 HOME、XDG、D-Bus 和显示器；不继承用户会话状态。"""
    parser = argparse.ArgumentParser()
    parser.add_argument('--inside', action='store_true')
    parser.add_argument('--application', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.inside:
        return inside(args.application, args.output)
    with tempfile.TemporaryDirectory(prefix='opspier-titlebar-') as directory:
        home = Path(directory)
        for part in ['config', 'data', 'cache', 'run']:
            (home / part).mkdir(mode=0o700)
        env = {k: v for k, v in os.environ.items() if k.startswith('OPSPIER_')}
        env.update(PATH='/usr/local/bin:/usr/bin:/bin', HOME=directory, DISPLAY=DISPLAY,
                   XDG_CONFIG_HOME=str(home / 'config'), XDG_DATA_HOME=str(home / 'data'),
                   XDG_CACHE_HOME=str(home / 'cache'), XDG_RUNTIME_DIR=str(home / 'run'),
                   XDG_DATA_DIRS=str(WM_ROOT / 'usr/share') + ':/usr/local/share:/usr/share',
                   LD_LIBRARY_PATH=str(WM_ROOT / 'usr/lib/x86_64-linux-gnu') + ':' + os.environ.get('LD_LIBRARY_PATH', ''),
                   LANG='C.UTF-8', LC_ALL='C.UTF-8', TAURI_WEBVIEW_AUTOMATION='true',
                   WEBKIT_DISABLE_COMPOSITING_MODE='1', GIO_USE_VFS='local', NO_PROXY='127.0.0.1,localhost')
        result = subprocess.run(['dbus-run-session', '--', '/usr/bin/python3', str(Path(__file__).resolve()), '--inside',
                                 '--application', str(args.application), '--output', str(args.output)], env=env, timeout=180)
    report_path = args.output / 'report.json'
    if report_path.exists():
        report = json.loads(report_path.read_text())
        report['cleanup']['privateHomeRemoved'] = not home.exists()
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    return result.returncode


if __name__ == '__main__':
    raise SystemExit(main())
