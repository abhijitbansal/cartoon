// Environment variables as the process sees them: any key may be missing.
export type Env = Record<string, string | undefined>;

export interface Config {
  host: string;
  port: number;
  debug: boolean;
  allowedOrigins: string[];
  admin: { name: string; email: string };
}

export interface User {
  id: number;
  name: string;
  email: string;
  roles: string[];
}
