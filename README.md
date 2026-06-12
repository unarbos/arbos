# Arbos

## Run anywhere

Web UI:

```bash
curl -fsSL https://arbos.life | arbos web
```

Terminal (TUI):

```bash
curl -fsSL https://arbos.life | arbos
```

First run on a machine without `arbos` on PATH — prefix either command:

```bash
arbos(){ bash -s "$@";};
```

Or install the launcher once: `curl -fsSL https://arbos.life/arbos -o ~/.local/bin/arbos && chmod +x ~/.local/bin/arbos`
