import {
  Client,
  Events,
  SlashCommandBuilder,
  REST,
  Routes,
  ChatInputCommandInteraction,
  ChannelType,
  TextChannel,
  ThreadChannel,
  MessageFlags,
  type Message,
} from "discord.js";
import type { Config, ChatEntry } from "./types.js";
import { DiscordSendQueue } from "./queue.js";
import {
  ensureWorkspace,
  ensureThread,
  resolveContext,
  appendChat,
  shouldRegenSummary,
  regenerateSummary,
  readPin,
  readBasePin,
  appendPin,
  listPins,
  clearPins,
  formatPinnedMessage,
  fillPlaceholders,
  buildPlaceholderVars,
  readGoal,
  writeGoal,
  readState,
  readRalphMeta,
  writeRalphMeta,
  threadIsInitialized,
  writeChannelCwd,
  resolveChannelCwd,
  buildChannelTopic,
} from "./workspace.js";
import { buildChannelPrompt, buildThreadChatPrompt } from "./prompt.js";
import { runAgent, isAgentRunning } from "./agent.js";
import {
  startLoop,
  pauseLoop,
  stopLoop,
  stopLoopsForChannel,
  setDelay,
  runSingleStep,
  getActiveLoops,
  resumeLoopsOnStartup,
} from "./ralph.js";
import { setVar, removeVar, listVars } from "./vault.js";
import { initMonitor, mlog, MONITOR_CHANNEL_NAME } from "./monitor.js";
import { resolve, join } from "path";
import { readFile, readdir, rm, stat } from "fs/promises";

// ── Slash Command Definitions ───────────────────────────────────────────────

const commands = [
  new SlashCommandBuilder()
    .setName("status")
    .setDescription("Show Arbos status and active loops"),

  new SlashCommandBuilder()
    .setName("pin")
    .setDescription("Show pins or append a new pin to this channel/thread")
    .addStringOption((o) =>
      o.setName("content").setDescription("New pin to append (omit to view all pins)")
    )
    .addBooleanOption((o) =>
      o.setName("clear").setDescription("Clear all operator-added pins")
    ),

  new SlashCommandBuilder()
    .setName("cwd")
    .setDescription("Show or set the working directory for this channel")
    .addStringOption((o) =>
      o.setName("path").setDescription("New CWD path relative to project root (omit to view)")
    ),

  new SlashCommandBuilder()
    .setName("thread")
    .setDescription("Create a RALPH loop thread")
    .addStringOption((o) =>
      o.setName("name").setDescription("Thread name").setRequired(true)
    )
    .addStringOption((o) =>
      o.setName("goal").setDescription("Goal for the thread").setRequired(true)
    ),

  new SlashCommandBuilder()
    .setName("close")
    .setDescription("Close this thread — stops the loop and deletes the thread"),

  new SlashCommandBuilder()
    .setName("pause")
    .setDescription("Pause the RALPH loop in this thread"),

  new SlashCommandBuilder()
    .setName("resume")
    .setDescription("Resume the RALPH loop in this thread"),

  new SlashCommandBuilder()
    .setName("delay")
    .setDescription("Set delay between RALPH steps")
    .addIntegerOption((o) =>
      o.setName("mins").setDescription("Minutes between steps").setRequired(true)
    ),

  new SlashCommandBuilder()
    .setName("step")
    .setDescription("Run a single RALPH step in this thread"),

  new SlashCommandBuilder()
    .setName("state")
    .setDescription("Show STATE.md for this thread"),

  new SlashCommandBuilder()
    .setName("goal")
    .setDescription("Show or update GOAL.md for this thread")
    .addStringOption((o) =>
      o.setName("content").setDescription("New goal (omit to view)")
    ),

  new SlashCommandBuilder()
    .setName("env")
    .setDescription("Manage encrypted environment variables")
    .addSubcommand((sub) =>
      sub
        .setName("set")
        .setDescription("Add or update an env var")
        .addStringOption((o) => o.setName("key").setDescription("Variable name").setRequired(true))
        .addStringOption((o) => o.setName("value").setDescription("Secret value").setRequired(true))
        .addStringOption((o) => o.setName("description").setDescription("What this var is for"))
    )
    .addSubcommand((sub) =>
      sub
        .setName("remove")
        .setDescription("Remove an env var")
        .addStringOption((o) => o.setName("key").setDescription("Variable name to remove").setRequired(true))
    )
    .addSubcommand((sub) =>
      sub.setName("list").setDescription("List all env vars (names and descriptions only)")
    ),
];

// ── Registration ────────────────────────────────────────────────────────────

export async function registerCommands(config: Config, clientId: string) {
  const rest = new REST({ version: "10" }).setToken(config.discordToken);
  await rest.put(Routes.applicationGuildCommands(clientId, config.guildId), {
    body: commands.map((c) => c.toJSON()),
  });
  mlog("info", "bot", `Slash commands registered`, {
    count: commands.length,
    names: commands.map((c) => c.name),
  });
}

// ── Event Wiring ────────────────────────────────────────────────────────────

export function wireEvents(client: Client, config: Config, queue: DiscordSendQueue) {
  client.on(Events.MessageCreate, (msg) => handleMessage(msg, config, queue));
  client.on(Events.ChannelCreate, (ch) => handleChannelCreate(ch as any, config, queue));
  client.on(Events.ChannelDelete, (ch) => handleChannelDelete(ch as any, config));
  client.on(Events.ThreadCreate, (thread, newlyCreated) =>
    handleThreadCreate(thread as any, newlyCreated, config)
  );
  client.on(Events.ThreadDelete, (thread) => handleThreadDelete(thread as any, config));
  client.on(Events.InteractionCreate, async (interaction) => {
    if (!interaction.isChatInputCommand()) return;
    if (interaction.guildId !== config.guildId) return;
    await handleCommand(interaction, config, queue);
  });
}

// ── Startup ─────────────────────────────────────────────────────────────────

async function cleanOrphanedWorkspace(client: Client, config: Config): Promise<void> {
  try {
    const guild = client.guilds.cache.get(config.guildId);
    if (!guild) return;

    const guildChannels = await guild.channels.fetch();
    const liveChannelNames = new Set<string>();
    const liveThreadNames = new Map<string, Set<string>>();
    const liveThreadsByName = new Map<string, Map<string, ThreadChannel>>();

    for (const [, ch] of guildChannels) {
      if (!ch || ch.type !== ChannelType.GuildText) continue;
      liveChannelNames.add(ch.name);
      const textCh = ch as TextChannel;
      const threadSet = new Set<string>();
      const threadMap = new Map<string, ThreadChannel>();

      const active = await textCh.threads.fetch();
      for (const [, t] of active.threads) {
        threadSet.add(t.name);
        threadMap.set(t.name, t);
      }

      try {
        const archived = await textCh.threads.fetchArchived({ fetchAll: true });
        for (const [, t] of archived.threads) {
          threadSet.add(t.name);
          threadMap.set(t.name, t);
        }
      } catch {}

      liveThreadNames.set(ch.name, threadSet);
      liveThreadsByName.set(ch.name, threadMap);
    }

    let entries: string[];
    try {
      entries = await readdir(config.workspaceRoot);
    } catch {
      return;
    }

    let removedChannels = 0;
    let removedThreads = 0;
    let closedThreads = 0;

    for (const entry of entries) {
      const entryPath = join(config.workspaceRoot, entry);
      const s = await stat(entryPath).catch(() => null);
      if (!s || !s.isDirectory()) continue;

      if (!liveChannelNames.has(entry)) {
        stopLoopsForChannel(entry);
        mlog("info", "cleanup", `Removing orphaned channel workspace`, { channel: entry });
        await rm(entryPath, { recursive: true, force: true });
        removedChannels++;
        continue;
      }

      const threadsDir = join(entryPath, "threads");
      let threadEntries: string[];
      try {
        threadEntries = await readdir(threadsDir);
      } catch {
        continue;
      }

      const liveSet = liveThreadNames.get(entry) ?? new Set();
      const threadMap = liveThreadsByName.get(entry) ?? new Map();

      for (const threadEntry of threadEntries) {
        const threadPath = join(threadsDir, threadEntry);
        const ts = await stat(threadPath).catch(() => null);
        if (!ts || !ts.isDirectory()) continue;

        if (!liveSet.has(threadEntry)) {
          for (const [tid, loop] of getActiveLoops()) {
            if (loop.channelName === entry && loop.threadName === threadEntry) {
              stopLoop(tid);
              break;
            }
          }
          mlog("info", "cleanup", `Removing orphaned thread workspace`, {
            channel: entry,
            thread: threadEntry,
          });
          await rm(threadPath, { recursive: true, force: true });
          removedThreads++;
          continue;
        }

        const meta = await readRalphMeta(threadPath);
        if (meta.state === "closed") {
          const discordThread = threadMap.get(threadEntry);
          if (discordThread) {
            mlog("info", "cleanup", `Deleting closed Discord thread`, {
              channel: entry,
              thread: threadEntry,
            });
            await discordThread.delete("Cleanup: thread was closed").catch((err: unknown) => {
              mlog("warn", "cleanup", `Failed to delete thread from Discord`, {
                channel: entry,
                thread: threadEntry,
                error: err instanceof Error ? err.message : String(err),
              });
            });
          }
          mlog("info", "cleanup", `Removing closed thread workspace`, {
            channel: entry,
            thread: threadEntry,
          });
          await rm(threadPath, { recursive: true, force: true });
          closedThreads++;
        }
      }
    }

    mlog("info", "cleanup", `Workspace cleanup complete`, {
      removedChannels,
      removedThreads,
      closedThreads,
    });
  } catch (err) {
    mlog("error", "cleanup", `Workspace cleanup failed`, {
      error: err instanceof Error ? err.message : String(err),
    });
  }
}

export async function onReady(client: Client, config: Config, queue: DiscordSendQueue) {
  mlog("info", "bot", `Logged in as ${client.user?.tag}`);

  await initMonitor(client, config);

  await ensureWorkspace(config, "root");

  const guild = client.guilds.cache.get(config.guildId);
  if (guild) {
    await assertRootChannel(guild, config);
  }

  await cleanOrphanedWorkspace(client, config);

  await resumeLoopsOnStartup(config, queue, async (channelName, threadName) => {
    if (!guild) return null;
    const channels = await guild.channels.fetch();
    for (const [, ch] of channels) {
      if (ch && ch.type === ChannelType.GuildText && ch.name === channelName) {
        const threads = await (ch as TextChannel).threads.fetch();
        for (const [id, thread] of threads.threads) {
          if (thread.name === threadName) {
            return { threadId: id, parentChannelId: ch.id };
          }
        }
      }
    }
    return null;
  });
}

async function assertRootChannel(guild: import("discord.js").Guild, config: Config) {
  const channels = await guild.channels.fetch();
  let rootChannel: TextChannel | null = null;

  for (const [, ch] of channels) {
    if (ch && ch.type === ChannelType.GuildText && ch.name === "root") {
      rootChannel = ch as TextChannel;
      break;
    }
  }

  if (!rootChannel) {
    mlog("info", "bot", `Root channel not found — creating #root`);
    rootChannel = await guild.channels.create({
      name: "root",
      type: ChannelType.GuildText,
      reason: "Arbos root channel",
    });
  }

  const pins = await rootChannel.messages.fetchPinned();
  if (pins.size > 0) return;

  mlog("info", "bot", `Pinning prompt banner in #root`);
  const ctx = await resolveContext(config, "root");
  const pinContent = await readPin(ctx.pinPath);
  const vars = buildPlaceholderVars(config, "root", { cwd: ctx.cwd });
  const filledPin = fillPlaceholders(pinContent, vars);
  const banner = formatPinnedMessage({
    cwd: ctx.cwd,
    channelName: "root",
    pin: filledPin,
  });

  try {
    const topic = buildChannelTopic(ctx.cwd);
    await rootChannel.setTopic(topic).catch(() => {});
    const pinMsg = await rootChannel.send(`\`\`\`\n${banner}\n\`\`\``);
    await pinMsg.pin();
  } catch (err) {
    mlog("error", "bot", `Failed to pin in #root`, {
      error: err instanceof Error ? err.message : String(err),
    });
  }
}

// ── Message Handler ─────────────────────────────────────────────────────────

async function handleMessage(msg: Message, config: Config, queue: DiscordSendQueue) {
  if (msg.author.bot) return;
  if (msg.system) return;
  if (!msg.guild || msg.guild.id !== config.guildId) return;

  const channel = msg.channel;
  if (!channel.isTextBased()) return;

  const isInThread = channel.isThread();
  const channelName = isInThread
    ? ((channel as ThreadChannel).parent as TextChannel)?.name ?? "root"
    : (channel as TextChannel).name;
  const threadName = isInThread ? (channel as ThreadChannel).name : undefined;

  mlog("info", "message", `Incoming message`, {
    author: msg.author.displayName ?? msg.author.username,
    channel: channelName,
    thread: threadName ?? null,
    contentLength: msg.content.length,
    contentPreview: msg.content.slice(0, 100),
  });

  await ensureWorkspace(config, channelName);

  if (isInThread && threadName && !(await threadIsInitialized(config, channelName, threadName))) {
    await initializeThreadRalph(msg, config, queue, channelName, threadName);
    return;
  }

  const ctx = await resolveContext(config, channelName, threadName);

  const entry: ChatEntry = {
    ts: new Date().toISOString(),
    role: "user",
    author: msg.author.displayName ?? msg.author.username,
    content: msg.content,
  };
  await appendChat(ctx.chatDir, entry);

  const prompt = isInThread
    ? await buildThreadChatPrompt(config, ctx)
    : await buildChannelPrompt(config, ctx);

  const contextKey = channel.id;
  if (isAgentRunning(contextKey)) {
    mlog("warn", "message", `Rejected — agent already running`, {
      channel: channelName,
      thread: threadName ?? null,
    });
    await queue.send(channel.id, "_Already working on a previous message…_");
    return;
  }

  const stream = await queue.createStream(channel.id);

  const result = await runAgent(contextKey, prompt, ctx.cwd, (chunk) => {
    stream.push(chunk);
  });

  await stream.finalize(result.output);

  await appendChat(ctx.chatDir, {
    ts: new Date().toISOString(),
    role: "assistant",
    content: result.output,
  });

  if (await shouldRegenSummary(ctx.chatDir)) {
    regenerateSummary(ctx.chatDir, config.openRouterKey);
  }
}

// ── Thread Auto-Init ────────────────────────────────────────────────────────

async function initializeThreadRalph(
  msg: Message,
  config: Config,
  queue: DiscordSendQueue,
  channelName: string,
  threadName: string
) {
  const goal = msg.content;
  const user = msg.author.displayName ?? msg.author.username;
  const thread = msg.channel as ThreadChannel;
  const parentChannelId = thread.parentId ?? thread.id;

  mlog("info", "ralph", `Auto-initializing thread`, {
    channelName,
    threadName,
    user,
    goal: goal.slice(0, 100),
  });

  await ensureThread(config, channelName, threadName, goal);

  const ctx = await resolveContext(config, channelName, threadName);
  const vars = buildPlaceholderVars(config, channelName, {
    threadName,
    threadDir: ctx.threadDir,
    cwd: ctx.cwd,
  });
  const ralphPath = resolve(config.workspaceRoot, "..", "RALPH.md");
  const ralph = fillPlaceholders(
    await readFile(ralphPath, "utf-8").catch(() => ""),
    vars,
  );
  const banner = formatPinnedMessage({
    cwd: ctx.cwd,
    channelName,
    threadName,
    threadDir: ctx.threadDir,
    goal,
    ralph,
  });
  const pinMsg = await thread.send(`\`\`\`\n${banner}\n\`\`\``);
  await pinMsg.pin().catch(() => {});

  await appendChat(ctx.chatDir, {
    ts: new Date().toISOString(),
    role: "system",
    content: `Thread created with goal: ${goal}`,
  });

  await startLoop(config, queue, thread.id, channelName, threadName, parentChannelId);
}

// ── Channel Create Handler ──────────────────────────────────────────────────

async function handleChannelCreate(
  channel: TextChannel,
  config: Config,
  queue: DiscordSendQueue
) {
  if (channel.guild?.id !== config.guildId) return;
  if (channel.type !== ChannelType.GuildText) return;
  if (channel.name === MONITOR_CHANNEL_NAME) return;

  mlog("info", "channel", `New channel created: #${channel.name}`);

  await ensureWorkspace(config, channel.name);
  const ctx = await resolveContext(config, channel.name);
  const pinContent = await readPin(ctx.pinPath);
  const vars = buildPlaceholderVars(config, channel.name, { cwd: ctx.cwd });
  const filledPin = fillPlaceholders(pinContent, vars);
  const banner = formatPinnedMessage({
    cwd: ctx.cwd,
    channelName: channel.name,
    pin: filledPin,
  });

  try {
    const topic = buildChannelTopic(ctx.cwd);
    await channel.setTopic(topic).catch(() => {});
    const pinMsg = await channel.send(`\`\`\`\n${banner}\n\`\`\``);
    await pinMsg.pin();
  } catch (err) {
    mlog("error", "channel", `Failed to pin in #${channel.name}`, {
      error: err instanceof Error ? err.message : String(err),
    });
  }
}

// ── Thread Create Handler ────────────────────────────────────────────────────

async function handleThreadCreate(
  thread: ThreadChannel,
  newlyCreated: boolean,
  config: Config
) {
  if (!newlyCreated) return;
  if (thread.guild?.id !== config.guildId) return;

  const parentChannel = thread.parent as TextChannel | null;
  if (!parentChannel) return;
  if (parentChannel.name === MONITOR_CHANNEL_NAME) return;

  const channelName = parentChannel.name;
  await ensureWorkspace(config, channelName);
  const cwd = await resolveChannelCwd(config, channelName);

  try {
    const header = await thread.send(`\`\`\`\nCWD: ${cwd}\n\`\`\``);
    await header.pin().catch(() => {});
  } catch (err) {
    mlog("error", "thread", `Failed to post header in thread ${thread.name}`, {
      error: err instanceof Error ? err.message : String(err),
    });
  }
}

// ── Channel / Thread Delete Handlers ────────────────────────────────────────

async function handleChannelDelete(channel: TextChannel, config: Config) {
  if (channel.guild?.id !== config.guildId) return;
  if (channel.type !== ChannelType.GuildText) return;

  const channelName = channel.name;
  mlog("info", "channel", `Channel deleted: #${channelName}`);

  const stopped = stopLoopsForChannel(channelName);
  if (stopped > 0) {
    mlog("info", "channel", `Stopped ${stopped} active loop(s) for deleted channel`, { channelName });
  }

  const channelDir = join(config.workspaceRoot, channelName);
  await rm(channelDir, { recursive: true, force: true }).catch((err: unknown) => {
    mlog("warn", "channel", `Failed to remove workspace for deleted channel`, {
      channelName,
      error: err instanceof Error ? err.message : String(err),
    });
  });
}

async function handleThreadDelete(thread: ThreadChannel, config: Config) {
  if (thread.guild?.id !== config.guildId) return;

  const threadName = thread.name;
  const parentChannel = thread.parent as TextChannel | null;
  const channelName = parentChannel?.name ?? "root";

  mlog("info", "thread", `Thread deleted: ${threadName}`, { channelName });

  stopLoop(thread.id);

  const threadDir = join(config.workspaceRoot, channelName, "threads", threadName);
  await rm(threadDir, { recursive: true, force: true }).catch((err: unknown) => {
    mlog("warn", "thread", `Failed to remove workspace for deleted thread`, {
      channelName,
      threadName,
      error: err instanceof Error ? err.message : String(err),
    });
  });
}

// ── Command Handler ─────────────────────────────────────────────────────────

async function handleCommand(
  interaction: ChatInputCommandInteraction,
  config: Config,
  queue: DiscordSendQueue
) {
  const cmd = interaction.commandName;
  const channel = interaction.channel;
  if (!channel) return;

  const isInThread = channel.isThread();
  const channelName = isInThread
    ? ((channel as ThreadChannel).parent as TextChannel)?.name ?? "root"
    : (channel as TextChannel).name;

  mlog("info", "command", `/${cmd} invoked`, {
    user: interaction.user.displayName ?? interaction.user.username,
    channel: channelName,
    isThread: isInThread,
  });

  try {
    switch (cmd) {
      case "status":
        await cmdStatus(interaction, config);
        break;
      case "pin":
        await cmdPin(interaction, config, channelName, isInThread ? channel.name : undefined);
        break;
      case "cwd":
        await cmdCwd(interaction, config, channelName);
        break;
      case "thread":
        await cmdThread(interaction, config, queue, channelName);
        break;
      case "close":
        await cmdClose(interaction, config, channelName, channel);
        break;
      case "pause":
        await cmdPause(interaction, config, channelName, channel);
        break;
      case "resume":
        await cmdResume(interaction, config, queue, channelName, channel);
        break;
      case "delay":
        await cmdDelay(interaction, config, channelName, channel);
        break;
      case "step":
        await cmdStep(interaction, config, queue, channelName, channel);
        break;
      case "state":
        await cmdState(interaction, config, channelName, channel);
        break;
      case "goal":
        await cmdGoal(interaction, config, channelName, channel);
        break;
      case "env":
        await cmdEnv(interaction);
        break;
      default:
        await interaction.reply({ content: "Unknown command.", flags: MessageFlags.Ephemeral });
    }
  } catch (err) {
    const errMsg = err instanceof Error ? err.message : "Unknown error";
    mlog("error", "command", `/${cmd} failed`, {
      error: errMsg,
      channel: channelName,
    });
    if (interaction.deferred || interaction.replied) {
      await interaction.editReply(`Error: ${errMsg}`);
    } else {
      await interaction.reply({ content: `Error: ${errMsg}`, flags: MessageFlags.Ephemeral });
    }
  }
}

// ── Individual Command Implementations ──────────────────────────────────────

async function cmdStatus(interaction: ChatInputCommandInteraction, config: Config) {
  const loops = getActiveLoops();
  if (loops.size === 0) {
    await interaction.reply("No active RALPH loops.");
    return;
  }
  const lines = Array.from(loops.values()).map(
    (l) => `• **${l.channelName}/${l.threadName}** (thread: ${l.threadId})`
  );
  await interaction.reply(`**Active loops:**\n${lines.join("\n")}`);
}

async function cmdPin(
  interaction: ChatInputCommandInteraction,
  config: Config,
  channelName: string,
  threadName?: string
) {
  const ctx = await resolveContext(config, channelName, threadName);
  const newContent = interaction.options.getString("content");
  const shouldClear = interaction.options.getBoolean("clear") ?? false;

  if (shouldClear) {
    const removed = await clearPins(ctx.pinPath);
    await interaction.reply(removed > 0
      ? `Cleared ${removed} operator pin(s). Base pin remains.`
      : `No operator pins to clear.`
    );
    return;
  }

  if (newContent) {
    const num = await appendPin(ctx.pinPath, newContent);
    await interaction.reply(`Pin #${num} added.`);
  } else {
    const base = await readBasePin(ctx.pinPath);
    const extras = await listPins(ctx.pinPath);
    const parts = [`**Base pin:**\n\`\`\`\n${base.trim()}\n\`\`\``];
    for (const p of extras) {
      parts.push(`**Pin #${p.id}:**\n\`\`\`\n${p.content.trim()}\n\`\`\``);
    }
    const full = parts.join("\n");
    const truncated = full.length > 1900 ? full.slice(0, 1900) + "\n…" : full;
    await interaction.reply(truncated);
  }
}

async function cmdCwd(
  interaction: ChatInputCommandInteraction,
  config: Config,
  channelName: string
) {
  const newPath = interaction.options.getString("path");

  if (newPath) {
    await ensureWorkspace(config, channelName);
    await writeChannelCwd(config, channelName, newPath);
    const resolved = await resolveChannelCwd(config, channelName);

    const ch = interaction.channel;
    if (ch && !ch.isThread() && "setTopic" in ch) {
      const topic = buildChannelTopic(resolved);
      await (ch as TextChannel).setTopic(topic).catch(() => {});
    }

    await interaction.reply(`CWD set to \`${resolved}\``);
  } else {
    const resolved = await resolveChannelCwd(config, channelName);
    await interaction.reply(`\`${resolved}\``);
  }
}

async function cmdThread(
  interaction: ChatInputCommandInteraction,
  config: Config,
  queue: DiscordSendQueue,
  channelName: string
) {
  const name = interaction.options.getString("name", true).replace(/\s+/g, "_");
  const goal = interaction.options.getString("goal", true);
  const user = interaction.user.displayName ?? interaction.user.username;

  // Ephemeral ack — nothing visible in the parent channel
  await interaction.deferReply({ flags: MessageFlags.Ephemeral });

  await ensureThread(config, channelName, name, goal);

  const parentChannel = interaction.channel as TextChannel;
  const thread = await parentChannel.threads.create({
    name,
    autoArchiveDuration: 10080, // 7 days
    reason: `RALPH loop: ${goal}`,
  });

  // 1. Post the user's command into the thread (no bot response)
  await thread.send(`**${user}:** /thread ${name} ${goal}`);

  // 2. Pin RALPH banner to the thread
  const ctx = await resolveContext(config, channelName, name);
  const vars = buildPlaceholderVars(config, channelName, {
    threadName: name,
    threadDir: ctx.threadDir,
    cwd: ctx.cwd,
  });
  const ralphPath = resolve(config.workspaceRoot, "..", "RALPH.md");
  const ralph = fillPlaceholders(
    await readFile(ralphPath, "utf-8").catch(() => ""),
    vars,
  );
  const banner = formatPinnedMessage({
    cwd: ctx.cwd,
    channelName,
    threadName: name,
    threadDir: ctx.threadDir,
    goal,
    ralph,
  });
  const pinMsg = await thread.send(`\`\`\`\n${banner}\n\`\`\``);
  await pinMsg.pin().catch(() => {});

  // Ephemeral confirmation only the invoker sees
  await interaction.editReply(`Thread **${name}** created.`);

  // Log system event
  await appendChat(ctx.chatDir, {
    ts: new Date().toISOString(),
    role: "system",
    content: `Thread created with goal: ${goal}`,
  });

  // 4. Start the RALPH loop immediately
  await startLoop(config, queue, thread.id, channelName, name, parentChannel.id);
}

function requireThread(channel: any): { threadName: string } {
  if (!channel.isThread()) {
    throw new Error("This command can only be used in a thread.");
  }
  return { threadName: channel.name };
}

async function cmdPause(
  interaction: ChatInputCommandInteraction,
  config: Config,
  channelName: string,
  channel: any
) {
  const { threadName } = requireThread(channel);
  await pauseLoop(config, channel.id, channelName, threadName);
  await interaction.reply("RALPH loop paused.");
}

async function cmdClose(
  interaction: ChatInputCommandInteraction,
  config: Config,
  channelName: string,
  channel: any
) {
  const { threadName } = requireThread(channel);
  await interaction.reply({ content: `Closing thread **${threadName}**…`, flags: MessageFlags.Ephemeral });

  await pauseLoop(config, channel.id, channelName, threadName);

  const threadDir = join(config.workspaceRoot, channelName, "threads", threadName);
  const meta = await readRalphMeta(threadDir);
  meta.state = "closed";
  await writeRalphMeta(threadDir, meta);

  await (channel as ThreadChannel).delete(`Closed via /close`);

  mlog("info", "bot", `Thread closed and deleted`, { channelName, threadName });
}

async function cmdResume(
  interaction: ChatInputCommandInteraction,
  config: Config,
  queue: DiscordSendQueue,
  channelName: string,
  channel: any
) {
  const { threadName } = requireThread(channel);
  const parentChannelId = (channel as ThreadChannel).parentId ?? channel.id;
  await startLoop(config, queue, channel.id, channelName, threadName, parentChannelId);
  await interaction.reply("RALPH loop resumed.");
}

async function cmdDelay(
  interaction: ChatInputCommandInteraction,
  config: Config,
  channelName: string,
  channel: any
) {
  const { threadName } = requireThread(channel);
  const mins = interaction.options.getInteger("mins", true);
  await setDelay(config, channelName, threadName, mins);
  await interaction.reply(`Delay set to **${mins}** minute(s).`);
}

async function cmdStep(
  interaction: ChatInputCommandInteraction,
  config: Config,
  queue: DiscordSendQueue,
  channelName: string,
  channel: any
) {
  const { threadName } = requireThread(channel);
  await interaction.deferReply();
  await runSingleStep(config, queue, channel.id, channelName, threadName);
  await interaction.editReply("Step complete.");
}

async function cmdState(
  interaction: ChatInputCommandInteraction,
  config: Config,
  channelName: string,
  channel: any
) {
  const { threadName } = requireThread(channel);
  const threadDir = join(config.workspaceRoot, channelName, "threads", threadName);
  const state = await readState(threadDir);
  const truncated = state.length > 1900 ? state.slice(0, 1900) + "\n…" : state;
  await interaction.reply(`\`\`\`\n${truncated}\n\`\`\``);
}

async function cmdGoal(
  interaction: ChatInputCommandInteraction,
  config: Config,
  channelName: string,
  channel: any
) {
  const { threadName } = requireThread(channel);
  const threadDir = join(config.workspaceRoot, channelName, "threads", threadName);
  const newContent = interaction.options.getString("content");

  if (newContent) {
    await writeGoal(threadDir, newContent);
    await interaction.reply("GOAL.md updated.");
  } else {
    const goal = await readGoal(threadDir);
    await interaction.reply(`**Goal:**\n${goal || "_No goal set._"}`);
  }
}

async function cmdEnv(interaction: ChatInputCommandInteraction) {
  const sub = interaction.options.getSubcommand();
  const user = interaction.user.displayName ?? interaction.user.username;

  switch (sub) {
    case "set": {
      const key = interaction.options.getString("key", true).toUpperCase();
      const value = interaction.options.getString("value", true);
      const description = interaction.options.getString("description") ?? "";
      await setVar(key, value, description, user);
      await interaction.reply({
        content: `\`${key}\` saved to vault.${description ? ` (${description})` : ""}`,
        flags: MessageFlags.Ephemeral,
      });
      break;
    }
    case "remove": {
      const key = interaction.options.getString("key", true).toUpperCase();
      const removed = await removeVar(key);
      await interaction.reply({
        content: removed ? `\`${key}\` removed from vault.` : `\`${key}\` not found in vault.`,
        flags: MessageFlags.Ephemeral,
      });
      break;
    }
    case "list": {
      const vars = await listVars();
      if (vars.length === 0) {
        await interaction.reply({ content: "Vault is empty.", flags: MessageFlags.Ephemeral });
        return;
      }
      const lines = vars.map(
        (v) => `• \`${v.key}\` — ${v.description || "_no description_"} (by ${v.addedBy})`
      );
      const body = `**Vault (${vars.length} var${vars.length === 1 ? "" : "s"}):**\n${lines.join("\n")}`;
      const truncated = body.length > 1900 ? body.slice(0, 1900) + "\n…" : body;
      await interaction.reply({ content: truncated, flags: MessageFlags.Ephemeral });
      break;
    }
  }
}
