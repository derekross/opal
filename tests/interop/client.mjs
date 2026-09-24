// Interop check: nostr-tools' BunkerSigner (what most web apps use) against
// Opal's signer. Run with `npm test` from this directory.
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import assert from 'node:assert/strict'
import { generateSecretKey, getPublicKey, verifyEvent } from 'nostr-tools/pure'
import * as nip44 from 'nostr-tools/nip44'
import { BunkerSigner, parseBunkerInput, createNostrConnectURI } from 'nostr-tools/nip46'
import { SimplePool } from 'nostr-tools/pool'

const bunker = spawn('cargo', ['run', '-q', '-p', 'opal-signer', '--example', 'dev-bunker'], {
  cwd: new URL('../..', import.meta.url).pathname,
  stdio: ['pipe', 'pipe', 'inherit'],
})
const lines = createInterface({ input: bunker.stdout })[Symbol.asyncIterator]()
const nextJson = async () => JSON.parse((await lines.next()).value)

const pool = new SimplePool()
let failed = false
const step = async (name, fn) => {
  try {
    await fn()
    console.log(`ok   ${name}`)
  } catch (e) {
    failed = true
    console.log(`FAIL ${name}: ${e.message}`)
  }
}

try {
  const { relay, bunker: uri, pubkey } = await nextJson()

  // bunker:// flow
  const bp = await parseBunkerInput(uri)
  const signer = BunkerSigner.fromBunker(generateSecretKey(), bp, { pool })
  await step('connect with client metadata', () =>
    signer.connect(JSON.stringify({ name: 'interop', url: 'https://example.com' })),
  )
  await step('ping', () => signer.ping()) // throws unless the reply is "pong"
  await step('get_public_key', async () => assert.equal(await signer.getPublicKey(), pubkey))
  await step('sign_event', async () => {
    const ev = await signer.signEvent({ kind: 1, content: 'hi', tags: [['t', 'opal']], created_at: Math.floor(Date.now() / 1000) })
    assert.equal(ev.pubkey, pubkey)
    assert.ok(verifyEvent(ev))
    assert.deepEqual(ev.tags, [['t', 'opal']])
  })
  await step('nip44 encrypt/decrypt', async () => {
    const peer = generateSecretKey()
    const ct = await signer.nip44Encrypt(getPublicKey(peer), 'secret')
    assert.equal(nip44.decrypt(ct, nip44.getConversationKey(peer, pubkey)), 'secret')
    assert.equal(await signer.nip44Decrypt(getPublicKey(peer), ct), 'secret')
  })
  await step('nip04 encrypt/decrypt', async () => {
    const peer = getPublicKey(generateSecretKey())
    assert.equal(await signer.nip04Decrypt(peer, await signer.nip04Encrypt(peer, 'old')), 'old')
  })
  await step('logout, then the connection is gone', async () => {
    await signer.logout() // throws unless the reply is "ack"
    const again = BunkerSigner.fromBunker(generateSecretKey(), bp, { pool })
    await assert.rejects(Promise.race([
      again.connect(),
      new Promise((_, rej) => setTimeout(() => rej(new Error('no answer')), 3000)),
    ]))
    await again.close()
  })

  // nostrconnect:// flow (modern name/perms params)
  await step('nostrconnect:// flow', async () => {
    const clientSk = generateSecretKey()
    const ncUri = createNostrConnectURI({
      clientPubkey: getPublicKey(clientSk),
      relays: [relay],
      secret: Math.random().toString(36).slice(2),
      name: 'Interop NC',
      perms: ['sign_event:1', 'nip44_encrypt'],
    })
    const pending = BunkerSigner.fromURI(clientSk, ncUri, { pool }, 10_000)
    await new Promise(r => setTimeout(r, 300))
    bunker.stdin.write(ncUri + '\n')
    const accepted = await nextJson()
    assert.equal(accepted.accepted, 'Interop NC')
    const nc = await pending
    assert.equal(await nc.getPublicKey(), pubkey)
    const ev = await nc.signEvent({ kind: 1, content: 'nc', tags: [], created_at: Math.floor(Date.now() / 1000) })
    assert.ok(verifyEvent(ev))
  })
} finally {
  bunker.kill()
  pool.destroy()
}
process.exit(failed ? 1 : 0)
