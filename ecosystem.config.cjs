module.exports = {
  apps: [{
    name: "arbos",
    script: "dist/index.js",
    cwd: __dirname,
    node_args: "--env-file=.env",
    max_restarts: 10,
    restart_delay: 3000,
    autorestart: true,
    watch: ["dist"],
    watch_delay: 1000,
    ignore_watch: ["node_modules", "logs", "workspace"],
    log_date_format: "YYYY-MM-DD HH:mm:ss.SSS",
    error_file: "logs/arbos-error.log",
    out_file: "logs/arbos-out.log",
    merge_logs: true,
    max_memory_restart: "512M",
  }]
};
