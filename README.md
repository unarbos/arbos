# Arbos

![Arbos](arbos.jpg)

<p align="center">
  Welcome! Arbos is simply a <a href="https://ghuntley.com/loop/">Ralph-loop</a> combined with a Telegram bot.<br>
  That's all you need to do just about anything.
</p>

# The Design

Arbos just loops a `GOAL.md` through a coding agent. 
```
                                     ┌────── [GOAL.md] ────────┐
                                     ▼                         │
                ┌──────────┐     ┌───────┐                     │
                │ Telegram │◄───►│ Agent │─────────────────────┘
                └──────────┘     └───────┘
```

## Requirements

- [Telegram Bot token](https://core.telegram.org/bots#how-do-i-create-a-bot)
- [Chutes API key](https://chutes.ai)

## Getting started

```sh
curl -fsSL https://raw.githubusercontent.com/unconst/Arbos/main/run.sh | bash
```

## Usage

To run Arbos just set the `/goal`:
```
/goal

Use the below program to evolve a system S that discovers profitable trading strategies.

You are given:
C = { Hyperliquid capital, Coinglass derivatives data (funding, OI, liquidations, leverage), compute on Basilica/Targon/Lium }

Initial state (build first)
S₀ = online continuous adaptive trading system which:
    - uses online data for training
    - uses evolutionary model search (mutate/replace weak models)
    - uses strict walk-forward validation + online Sharpe filtering
    - uses horizon ensembles H = {1h,4h,8h,12h,24h}
    - uses consensus gating for signals
    - uses time-series foundation models

Run this loop continously
loop t = 1..∞
    S_t = design_or_modify(S_{t-1})  # implement current design S
    O_t = run(S_t)                   # run S: i.e. train, evaluate, trade
    P_t = measure(O_t)               # eval: Sharpe, PnL, drawdown, regime behavior
    Δ_t = reflect(S_t, P_t)          # find weaknesses in your design 
    S_{t+1} = improve(S_t, Δ_t)      # design a new design.
end
```

Then iterate.

---

MIT 

