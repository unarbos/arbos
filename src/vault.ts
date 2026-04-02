import { randomBytes, createCipheriv, createDecipheriv, scryptSync } from "crypto";
import { mkdir, readFile, writeFile } from "fs/promises";
import { join } from "path";
import { homedir } from "os";

const VAULT_DIR = join(homedir(), ".arbos");
const VAULT_PATH = join(VAULT_DIR, "vault.enc");
const ALGORITHM = "aes-256-gcm";
const IV_LEN = 16;
const TAG_LEN = 16;

export interface VaultEntry {
  value: string;
  description: string;
  addedBy: string;
  addedAt: string;
}

export type VaultData = Record<string, VaultEntry>;

function deriveKey(passphrase: string): Buffer {
  return scryptSync(passphrase, "arbos-vault-salt", 32);
}

function getVaultKey(): Buffer {
  const raw = process.env.VAULT_KEY;
  if (!raw) throw new Error("VAULT_KEY not set");
  return deriveKey(raw);
}

function encrypt(plaintext: string, key: Buffer): Buffer {
  const iv = randomBytes(IV_LEN);
  const cipher = createCipheriv(ALGORITHM, key, iv);
  const encrypted = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
  const tag = cipher.getAuthTag();
  // layout: [iv (16)] [tag (16)] [ciphertext (...)]
  return Buffer.concat([iv, tag, encrypted]);
}

function decrypt(blob: Buffer, key: Buffer): string {
  const iv = blob.subarray(0, IV_LEN);
  const tag = blob.subarray(IV_LEN, IV_LEN + TAG_LEN);
  const ciphertext = blob.subarray(IV_LEN + TAG_LEN);
  const decipher = createDecipheriv(ALGORITHM, key, iv);
  decipher.setAuthTag(tag);
  return Buffer.concat([decipher.update(ciphertext), decipher.final()]).toString("utf8");
}

export async function loadVault(): Promise<VaultData> {
  try {
    const blob = await readFile(VAULT_PATH);
    const json = decrypt(blob, getVaultKey());
    return JSON.parse(json) as VaultData;
  } catch (err: any) {
    if (err?.code === "ENOENT") return {};
    throw err;
  }
}

export async function saveVault(data: VaultData): Promise<void> {
  await mkdir(VAULT_DIR, { recursive: true });
  const json = JSON.stringify(data, null, 2);
  const blob = encrypt(json, getVaultKey());
  await writeFile(VAULT_PATH, blob);
}

export async function setVar(
  key: string,
  value: string,
  description: string,
  addedBy: string
): Promise<void> {
  const data = await loadVault();
  data[key] = { value, description, addedBy, addedAt: new Date().toISOString() };
  await saveVault(data);
}

export async function removeVar(key: string): Promise<boolean> {
  const data = await loadVault();
  if (!(key in data)) return false;
  delete data[key];
  await saveVault(data);
  return true;
}

export async function listVars(): Promise<
  Array<{ key: string; description: string; addedBy: string; addedAt: string }>
> {
  const data = await loadVault();
  return Object.entries(data).map(([key, entry]) => ({
    key,
    description: entry.description,
    addedBy: entry.addedBy,
    addedAt: entry.addedAt,
  }));
}

export async function getAllValues(): Promise<Record<string, string>> {
  const data = await loadVault();
  const out: Record<string, string> = {};
  for (const [key, entry] of Object.entries(data)) {
    out[key] = entry.value;
  }
  return out;
}

export async function getManifest(): Promise<string> {
  const vars = await listVars();
  if (vars.length === 0) return "";
  const lines = vars.map((v) => `- ${v.key}: ${v.description}`);
  return [
    "Available environment variables (access via $VAR_NAME in shell or process.env):",
    ...lines,
  ].join("\n");
}
