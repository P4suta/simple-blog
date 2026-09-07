import { existsSync, renameSync, rmSync } from 'node:fs';
import { resolve, dirname, join, sep } from 'node:path';
import { randomUUID } from 'node:crypto';

export function publishEvidence(staging, destination, move = renameSync, remove = rmSync) {
  const staged = resolve(staging), published = resolve(destination), parent = dirname(published);
  if (staged === published || dirname(staged) !== parent || parent === published) throw new Error('Evidence publication requires distinct sibling directories');
  const previous = join(parent, `.shareable-previous-${randomUUID()}`);
  if (!previous.startsWith(parent + sep)) throw new Error('Evidence cleanup escaped its parent');
  const hadPrevious = existsSync(published);
  if (hadPrevious) move(published, previous);
  try { move(staged, published); }
  catch (error) {
    if (hadPrevious) move(previous, published);
    throw error;
  }
  // The previous validated export is recoverable until the new one is installed.
  if (hadPrevious) remove(previous, { recursive: true, force: true });
}
