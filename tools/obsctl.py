"""Drive OBS through its websocket for test recordings.

    work/venv/Scripts/python tools/obsctl.py info|setup|start|stop|shot <out.png>

Needs OBS with its websocket server enabled and obsws-python in the venv. `setup` creates scene
`ts3-test` with capture method 2 (Windows Graphics Capture); the default records white on some setups.
"""
import json, os, sys, obsws_python as obs
pw = json.load(open(os.path.expandvars(r'%APPDATA%\obs-studio\plugin_config\obs-websocket\config.json')))['server_password']
c = obs.ReqClient(host='127.0.0.1', port=4455, password=pw, timeout=10)
cmd = sys.argv[1]
if cmd == 'info':
    print(c.get_version().obs_version, [s['sceneName'] for s in c.get_scene_list().scenes], c.get_record_status().output_active)
    print(c.get_video_settings().__dict__)
elif cmd == 'setup':
    names = [s['sceneName'] for s in c.get_scene_list().scenes]
    if 'ts3-test' not in names:
        c.create_scene('ts3-test')
        c.create_input('ts3-test', 'ts3-display', 'monitor_capture', {'method': 2}, True)
        items = c.get_input_properties_list_property_items('ts3-display', 'monitor_id').property_items
        mon = next((i['itemValue'] for i in items if i['itemEnabled']), None)
        if mon:
            c.set_input_settings('ts3-display', {'method': 2, 'monitor_id': mon}, True)
    c.set_current_program_scene('ts3-test')
    print(c.get_input_settings('ts3-display').input_settings)
elif cmd == 'start':
    c.start_record(); print('recording')
elif cmd == 'stop':
    print(c.stop_record().output_path)
elif cmd == 'shot':
    r = c.save_source_screenshot('ts3-test', 'png', sys.argv[2], 1280, 720, -1); print('saved')
