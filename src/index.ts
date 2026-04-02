import { Client, GatewayIntentBits, Events } from "discord.js";
import { config as loadEnv } from "dotenv";
import { resolve, join } from "path";
import type { Config } from "./types.js";
import { DiscordSendQueue } from "./queue.js";
import { registerCommands, wireEvents, onReady } from "./bot.js";
import { destroyMonitor, mlog, forceFlush } from "./monitor.js";
import { getActiveLoops } from "./ralph.js";
import { readRalphMeta } from "./workspace.js";
import { getAllValues } from "./vault.js";

loadEnv();

// ── Config ──────────────────────────────────────────────────────────────────

async function loadConfig(): Promise<Config> {
  const vaultKey = process.env.VAULT_KEY;
  if (!vaultKey) throw new Error("VAULT_KEY is required");

  const vaultVars = await getAllValues();
  const get = (key: string) => process.env[key] || vaultVars[key];

  const discordToken = get("DISCORD_TOKEN");
  const guildId = get("GUILD_ID");
  const openRouterKey = get("OPENROUTER_KEY");
  const workspaceRoot = resolve(process.env.WORKSPACE_ROOT ?? "./workspace");

  if (!discordToken) throw new Error("DISCORD_TOKEN is required (set in vault or env)");
  if (!guildId) throw new Error("GUILD_ID is required (set in vault or env)");
  if (!openRouterKey) throw new Error("OPENROUTER_KEY is required (set in vault or env)");

  return { discordToken, guildId, openRouterKey, workspaceRoot, vaultKey };
}

// ── Boot ────────────────────────────────────────────────────────────────────

async function main() {
  const config = await loadConfig();

  const client = new Client({
    intents: [
      GatewayIntentBits.Guilds,
      GatewayIntentBits.GuildMessages,
      GatewayIntentBits.MessageContent,
    ],
  });

  const queue = new DiscordSendQueue(client);

  client.once(Events.ClientReady, async (readyClient) => {
    await registerCommands(config, readyClient.user.id);
    await onReady(client, config, queue);
  });

  wireEvents(client, config, queue);

  await client.login(config.discordToken);

  const shutdown = async () => {
    mlog("warn", "arbos", "Shutting down…");

    const loops = getActiveLoops();
    for (const [, loop] of loops) {
      try {
        const threadDir = join(config.workspaceRoot, loop.channelName, "threads", loop.threadName);
        const meta = await readRalphMeta(threadDir);
        if (meta.currentStep != null) {
          await queue.send(loop.threadId,
            `**— Step ${meta.currentStep} —**\n_Interrupted — process shutting down._`
          );
          await queue.send(loop.parentChannelId,
            `**${loop.threadName}** — killed (process restart)`
          );
        }
      } catch {}
    }

    destroyMonitor();
    await forceFlush().catch(() => {});
    queue.destroy();
    client.destroy();
    process.exit(0);
  };
  process.on("SIGINT", () => shutdown());
  process.on("SIGTERM", () => shutdown());
}

main().catch((err) => {
  mlog("error", "arbos", `Fatal: ${err instanceof Error ? err.message : String(err)}`);
  console.error("[arbos] Fatal:", err);
  process.exit(1);
});
