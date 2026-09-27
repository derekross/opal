.pragma library

function ago(secs, nowMs) {
  if (!secs) return "never"
  var d = Math.max(0, Math.floor((nowMs || Date.now()) / 1000) - secs)
  if (d < 60) return "just now"
  if (d < 3600) return Math.floor(d / 60) + "m ago"
  if (d < 86400) return Math.floor(d / 3600) + "h ago"
  if (d < 86400 * 30) return Math.floor(d / 86400) + "d ago"
  return new Date(secs * 1000).toLocaleDateString()
}

function clock(secs) {
  var t = new Date(secs * 1000)
  var today = new Date()
  var hm = ("0" + t.getHours()).slice(-2) + ":" + ("0" + t.getMinutes()).slice(-2)
  return t.toDateString() === today.toDateString() ? hm : (t.getMonth() + 1) + "/" + t.getDate() + " " + hm
}

function shortKey(s) {
  if (!s) return ""
  return s.length > 20 ? s.slice(0, 12) + "…" + s.slice(-6) : s
}

var methodLabels = {
  get_public_key: "Read your public key",
  sign_event: "Sign an event",
  nip04_encrypt: "Encrypt (NIP-04)",
  nip04_decrypt: "Decrypt (NIP-04)",
  nip44_encrypt: "Encrypt (NIP-44)",
  nip44_decrypt: "Decrypt (NIP-44)"
}

function describe(method, kindLabel) {
  if (method === "sign_event" && kindLabel) return "Sign: " + kindLabel
  return methodLabels[method] || method
}

var policyOptions = [
  { value: "basic", label: "Basic", tooltip: "Approve everyday actions automatically; ask for the rest" },
  { value: "manual", label: "Ask", tooltip: "Ask for everything" },
  { value: "full-trust", label: "Trust", tooltip: "Approve everything this app asks" }
]

var rememberOptions = [
  { value: "once", label: "Once" },
  { value: "5m", label: "5m" },
  { value: "1h", label: "1h" },
  { value: "1d", label: "1d" },
  { value: "1w", label: "1w" },
  { value: "always", label: "Always" }
]

var sourceLabels = {
  full_trust: "full trust",
  rule: "saved rule",
  basic_policy: "basic policy",
  user: "you",
  timeout: "timed out",
  locked: "locked",
  automatic: "automatic",
  error: "error"
}
