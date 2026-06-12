/**
 * One shared /api/activity poller for the whole window. Every consumer
 * (the activity tab, each run tab's status check) used to run its own
 * identical 5s interval — N tabs = N+1 polls — and kept polling with the
 * window hidden. One subscription fans the same response out to everyone,
 * and the interval pauses entirely while the document is hidden (resuming
 * with an immediate tick on visibility).
 */
import { useEffect, useState } from "react";

import { fetchActivity, type Activity } from "./api";

const POLL_MS = 5000;

type Sub = (a: Activity) => void;
const subs = new Set<Sub>();
let timer: number | undefined;
let inflight = false;
let last: Activity | null = null;
let listening = false;

async function tick() {
  if (inflight) return;
  inflight = true;
  try {
    const a = await fetchActivity();
    last = a;
    for (const s of subs) s(a);
  } catch {
    // transient fetch failure: keep the previous snapshot, next tick retries
  } finally {
    inflight = false;
  }
}

function running(): boolean {
  return timer !== undefined;
}

function start() {
  if (running() || subs.size === 0 || document.visibilityState !== "visible") return;
  void tick();
  timer = window.setInterval(() => void tick(), POLL_MS);
}

function stop() {
  if (timer !== undefined) {
    window.clearInterval(timer);
    timer = undefined;
  }
}

function onVisibility() {
  if (document.visibilityState === "visible") start();
  else stop();
}

/** Subscribe to the shared activity feed; the latest snapshot (if any) is
 *  delivered synchronously. Returns the unsubscribe. */
export function subscribeActivity(fn: Sub): () => void {
  subs.add(fn);
  if (last) fn(last);
  if (!listening) {
    listening = true;
    document.addEventListener("visibilitychange", onVisibility);
  }
  start();
  return () => {
    subs.delete(fn);
    if (subs.size === 0) stop();
  };
}

/** The shared activity snapshot as a hook (null until the first poll lands). */
export function useActivity(): Activity | null {
  const [activity, setActivity] = useState<Activity | null>(last);
  useEffect(() => subscribeActivity(setActivity), []);
  return activity;
}
