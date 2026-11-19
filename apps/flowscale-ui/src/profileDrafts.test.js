import {test} from 'node:test';
import assert from 'node:assert/strict';
import {acknowledgeProfileDraft, mergePendingProfiles, takePendingProfiles} from './profileDrafts.js';

test('early state sync keeps the edited profile until its debounce flush', () => {
  const oldProfile = {id: 'game', title: 'old'};
  const draft = {id: 'game', title: 'edited'};
  const pending = new Map([['game', {timerId: 1, profile: draft}]]);
  const inFlight = new Map();
  const visible = mergePendingProfiles([oldProfile], pending, inFlight);
  assert.deepEqual(visible, [draft]);
  assert.equal(pending.get('game').profile, draft);
  inFlight.set('game', {revision: '1', profile: draft});
  takePendingProfiles(pending);
  assert.deepEqual(mergePendingProfiles([oldProfile], pending, inFlight), [draft]);
});

test('delete removes an in-flight profile before the next snapshot', () => {
  const existing = [{id: 'default', title: 'Default'}];
  const draft = {id: 'new', title: 'New'};
  const pending = new Map([['new', {timerId: 2, profile: draft}]]);
  assert.deepEqual(mergePendingProfiles(existing, pending), [...existing, draft]);
  const inFlight = new Map([['new', {revision: '2', profile: draft}]]);
  pending.delete('new');
  assert.deepEqual(mergePendingProfiles(existing, pending, inFlight), [...existing, draft]);
  inFlight.delete('new');
  assert.deepEqual(mergePendingProfiles(existing, pending, inFlight), existing);
});

test('older save acknowledgements leave the newer same-ID draft in flight', () => {
  const oldDraft = {id: 'game', title: 'revision 1'};
  const newDraft = {id: 'game', title: 'revision 2'};
  const inFlight = new Map([['game', {revision: '2', profile: newDraft}]]);
  assert.equal(acknowledgeProfileDraft(inFlight, 'game', '1'), false);
  assert.deepEqual(mergePendingProfiles([{id: 'game', title: 'host'}], new Map(), inFlight), [newDraft]);
  assert.equal(acknowledgeProfileDraft(inFlight, 'game', '2'), true);
  assert.deepEqual(mergePendingProfiles([{id: 'game', title: 'host'}], new Map(), inFlight), [{id: 'game', title: 'host'}]);
});

test('reset and rejected-hotkey snapshots preserve pending durable acknowledgements', () => {
  const draft = {id: 'game', title: 'edited'};
  const inFlight = new Map([['game', {revision: '5', profile: draft}]]);
  const hostSnapshot = [{id: 'game', title: 'old'}];
  assert.deepEqual(mergePendingProfiles(hostSnapshot, new Map(), inFlight), [draft]);
  assert.deepEqual(mergePendingProfiles(hostSnapshot, new Map(), inFlight), [draft]);
});
