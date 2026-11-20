"""Autopilot workloads shared by baseline and candidate."""


def home_steps():
    return [
        {"phase": "startup"}, {"wait_mark": "home_loaded", "timeout": 8},
        {"phase": "settling"}, {"sleep": 2},
        {"phase": "idle"}, {"sleep": 3},
    ]


def steps_for(mode):
    home = home_steps()
    browse = home + [
        {"phase": "wheel"},
        {"repeat": 15, "every": 0.1, "do": {"cmd": "scroll", "ticks": 1}},
        {"repeat": 12, "every": 0.1, "do": {"cmd": "scroll", "ticks": -1}},
        {"phase": "fling"}, {"cmd": "fling", "velocity": 2500}, {"sleep": 1.5},
        {"cmd": "scroll_to_top"}, {"phase": "recovered"}, {"sleep": 3},
    ]
    play = home + [
        {"phase": "open"}, *open_video(1),
        {"phase": "playing"}, {"sleep": 20 if mode == "soak" else 6},
        {"phase": "switch"}, *open_video(0),
        {"phase": "switched"}, {"sleep": 3},
        {"phase": "closing"}, {"cmd": "back"}, {"sleep": 0.5}, {"cmd": "back"},
        {"sleep": 1}, {"phase": "recovered"}, {"sleep": 3},
    ]
    navigation = home.copy()
    for cycle in range(1, 17 if mode == "soak" else 9):
        navigation += [
            {"phase": f"visit_{cycle:02d}"}, *open_video(1, timeout=5), {"sleep": 0.6},
            {"cmd": "back"}, {"sleep": 0.6},
            {"phase": f"recovered_{cycle:02d}"}, {"sleep": 1.6},
        ]
    return {"load": home, "browse": browse, "play": play, "navigation": navigation}


def open_video(index, timeout=8):
    return [
        {"cmd": "open_video", "index": index},
        {"wait_mark": "watch_loaded", "timeout": timeout},
        {"wait_mark": "playback_started", "timeout": timeout},
    ]


def live_steps():
    # Two results from one query keep request volume low and avoid following
    # a new recommendations chain on each backend.
    return home_steps() + [
        {"phase": "wheel"},
        {"repeat": 10, "every": 0.12, "do": {"cmd": "scroll", "ticks": 1}},
        {"repeat": 10, "every": 0.12, "do": {"cmd": "scroll", "ticks": -1}},
        {"phase": "search"}, {"cmd": "search", "query": "Big Buck Bunny Blender Foundation"},
        {"wait_mark": "search_loaded", "timeout": 8}, {"sleep": 1},
        {"phase": "open"}, *open_video(0, timeout=10),
        {"phase": "playing"}, {"sleep": 7},
        {"phase": "switch"}, {"cmd": "back"}, {"sleep": 0.7}, *open_video(1, timeout=10),
        {"phase": "switched"}, {"sleep": 7},
        {"phase": "closing"}, {"cmd": "back"}, {"sleep": 0.7}, {"cmd": "back"},
        {"sleep": 1}, {"phase": "recovered"}, {"sleep": 3},
    ]
