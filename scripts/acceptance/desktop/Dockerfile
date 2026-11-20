FROM ubuntu:24.04

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        dbus-daemon fonts-noto-color-emoji fonts-wqy-zenhei ibus ibus-gtk3 ibus-libpinyin \
        openbox python3 scrot x11-utils xauth xclip xdotool xfce4-terminal xvfb \
    && rm -rf /var/lib/apt/lists/*

ENV HOME=/tmp \
    LANG=C.UTF-8 \
    GDK_BACKEND=x11 \
    GSETTINGS_BACKEND=memory \
    GTK_IM_MODULE=ibus \
    PYTHONDONTWRITEBYTECODE=1 \
    XMODIFIERS=@im=ibus

COPY scenario.py /test/desktop.py
ENTRYPOINT ["dbus-run-session", "--", "python3", "/test/desktop.py"]
