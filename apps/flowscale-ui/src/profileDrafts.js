export function takePendingProfiles(pending) {
  const entries = Array.from(pending.values());
  pending.clear();
  return entries;
}

export function acknowledgeProfileDraft(inFlight, id, revision) {
  if (inFlight.get(id)?.revision !== revision) return false;
  inFlight.delete(id);
  return true;
}

export function mergePendingProfiles(snapshot, pending, inFlight = new Map()) {
  const profiles = snapshot.slice();
  const indexes = new Map(profiles.map((profile, index) => [profile.id, index]));
  for (const [id, {profile}] of [...inFlight, ...pending]) {
    const index = indexes.get(id);
    if (index === undefined) {
      indexes.set(id, profiles.length);
      profiles.push(profile);
    } else {
      profiles[index] = profile;
    }
  }
  return profiles;
}
