# App sandbox

The released Herdr app, headless in a Docker container, for trying the kiosk mod with
real key presses and clicks and recording it, without touching anyone's screen. The
container has an Xvfb display, Mesa's software Vulkan, `xdotool` for input, and the app
from its own installer. This checkout is mounted at `/src`.

```sh
scripts/app-sandbox/run.sh e2e      # the end-to-end suite
scripts/app-sandbox/run.sh shots    # every screen kiosk draws, in target/shots
scripts/app-sandbox/run.sh record   # media/preview.gif
scripts/app-sandbox/run.sh up       # then any `sandbox` command; see the top of `sandbox`
scripts/app-sandbox/run.sh rm       # delete the container
```

`run.sh` builds the image on first use. `SANDBOX_THEME` picks one of `themes/*.json`, or a
theme the app ships such as `paper`, for `up`, `e2e` and `shots` (the recording uses
`catppuccin-macchiato`), `SANDBOX_APPEARANCE` light or dark, and `SANDBOX_SCALE` 2 draws the
window at twice its size, as a high-density screen would (`shots` and the recording do).
`shots` saves the same screens every run, for comparing how two versions of the mod look;
`KIOSK_MOD` points `install` at another copy of the mod's folder to draw them with. `KIOSK_SANDBOX_MOUNTS` adds `docker run` volume
arguments, for example a local kiosk checkout that `Cargo.toml` patches in, mounted where
its relative path points. Screenshots and the suite's pictures land in `target/` of this
checkout.
