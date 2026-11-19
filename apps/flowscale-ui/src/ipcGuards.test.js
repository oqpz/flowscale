import {test} from 'node:test';
import assert from 'node:assert/strict';
import {isHostEventShape} from './ipcGuards.js';

test('IPC accepts known event payload shapes and rejects malformed or unknown events', () => {
  assert.equal(isHostEventShape({type: 'countdown_tick', payload: {seconds: 3}}), true);
  assert.equal(isHostEventShape({type: 'countdown_tick', payload: {seconds: '3'}}), false);
  assert.equal(isHostEventShape({type: 'toast', payload: {kind: 'error', message: 'bad'}}), true);
  assert.equal(isHostEventShape({type: 'toast', payload: {kind: 'other', message: 'bad'}}), false);
  assert.equal(isHostEventShape({type: 'profile_upserted', payload: {id: 'game', revision: '7', ok: true, profile: {id: 'game'}}}), true);
  assert.equal(isHostEventShape({type: 'profile_upserted', payload: {id: 'game', revision: '7', ok: true}}), false);
  assert.equal(isHostEventShape({type: 'unrecognized', payload: {}}), false);
});

test('config responses distinguish updates from cancellation and reject invalid entries', () => {
  const accepts = entries => isHostEventShape({type: 'configs_result', payload: {entries}});
  assert.equal(accepts(null), true);
  assert.equal(accepts([{id: 1, name: 'Example'}]), true);
  assert.equal(accepts([{id: '1', name: 'Example'}]), false);
  assert.equal(accepts([{id: 1, name: 1}]), false);
});
