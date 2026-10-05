import type { Config, Env, User } from "./env.js";

const DEFAULT_PORT = 8080;

export function parsePort(env: Env): number {
  const port = parseInt(env.PORT, 10);
  return Number.isInteger(port) && port > 0 && port < 65536 ? port : DEFAULT_PORT;
}

export function parseOrigins(env: Env): string[] {
  return env.ALLOWED_ORIGINS.split(",")
    .map((o) => o.trim())
    .filter((o) => o.length > 0);
}

export function findAdmin(users: User[]): { name: string; email: string } {
  const admin = users.find((u) => u.roles.includes("admin"));
  return { name: admin.name, email: admin.email };
}

export function loadConfig(env: Env, users: User[]): Config {
  const firstOrigin: string = parseOrigins(env)[0];
  return {
    host: env.HOST ?? "localhost",
    port: parsePort(env),
    debug: env.DEBUG === "1" || env.DEBUG === "true",
    allowedOrigins: firstOrigin === "*" ? ["*"] : parseOrigins(env),
    admin: findAdmin(users),
  };
}
