import QtQuick
import Quickshell
import Quickshell.Io

// One connection to opald for the whole shell. The bar widget (one per
// monitor) and the approval overlay read state from here and send requests
// through call().
Item {
  id: root

  // Injected by omarchy-shell.
  property var shell: null
  property var manifest: null

  readonly property string pluginId: (manifest && manifest.id) || "opal"
  readonly property string socketPath: Quickshell.env("XDG_RUNTIME_DIR") + "/opal.sock"

  readonly property bool connected: sock.connected
  property bool everConnected: false

  // Daemon state (see opald `status`).
  property var status: ({})
  readonly property bool locked: status.locked !== false
  readonly property bool hasAccounts: status.has_accounts === true
  readonly property var identity: status.identity || ({})
  readonly property bool readOnly: identity.mode === "read-only" && !!identity.npub
  // Something to show: a local key, or an npub being watched.
  readonly property bool configured: hasAccounts || readOnly
  readonly property bool online: status.online !== false
  readonly property var accounts: status.accounts || []
  readonly property var currentAccount: {
    for (var i = 0; i < accounts.length; i++) if (accounts[i].current) return accounts[i]
    return accounts.length > 0 ? accounts[0] : null
  }

  property var apps: []
  property var notifications: []
  property var statusInfo: ({})
  property var plays: []
  property var playStats: ({})
  readonly property bool statusOn: !!status.modules && status.modules.status === true
  property var notifyStatus: ({})
  readonly property int unread: status.unread_notifications || 0
  readonly property bool notificationsOn: !!status.modules && status.modules.notifications === true
  property var prompts: []
  property var offers: []
  property var activity: []
  property var stats: ({})
  property var config: ({})
  // An app is waiting for an unlock (cleared once unlocked).
  property var unlockRequest: null
  // The last bunker URI created from the panel, shown until dismissed.
  property var lastBunker: null

  readonly property int attentionCount: prompts.length + offers.length + (unlockRequest && locked ? 1 : 0)

  signal message(string text, bool isError)
  signal panelToggleRequested()
  signal tabRequested(string name)

  property int _nextId: 1
  property var _callbacks: ({})

  function call(method, params, done) {
    if (!sock.connected) {
      if (done) done("Opal isn't running", null)
      return
    }
    var id = _nextId++
    if (done) _callbacks[id] = done
    sock.write(JSON.stringify({ id: id, method: method, params: params === undefined ? null : params }) + "\n")
    sock.flush()
  }

  // call() with a toast on error and an optional success callback.
  function run(method, params, onOk) {
    call(method, params, function(err, result) {
      if (err) root.message(err, true)
      else if (onOk) onOk(result)
    })
  }

  function refreshApps() { call("apps.list", null, function(e, r) { if (!e) root.apps = r || [] }) }
  function refreshPrompts() { call("prompts.list", null, function(e, r) { if (!e) root.prompts = r || [] }) }
  function refreshOffers() { call("nostrconnect.offers", null, function(e, r) { if (!e) root.offers = r || [] }) }
  function refreshConfig() { call("config.get", null, function(e, r) { if (!e) root.config = r || {} }) }
  function refreshActivity() {
    call("activity.list", { limit: 200 }, function(e, r) { if (!e) root.activity = r || [] })
    call("activity.stats", null, function(e, r) { if (!e) root.stats = r || {} })
  }
  function refreshNotifications() {
    call("notifications.list", { limit: 150 }, function(e, r) { if (!e) root.notifications = r || [] })
    call("notifications.status", null, function(e, r) { if (!e) root.notifyStatus = r || {} })
  }
  function refreshStatus() {
    call("status.get", null, function(e, r) { if (!e) root.statusInfo = r || {} })
    call("scrobbles.recent", { limit: 30 }, function(e, r) { if (!e) root.plays = r || [] })
    call("scrobbles.stats", null, function(e, r) { if (!e) root.playStats = r || {} })
  }
  function refreshAll() {
    refreshApps(); refreshPrompts(); refreshOffers(); refreshActivity(); refreshConfig(); refreshNotifications(); refreshStatus()
  }

  function showApproval() {
    if (shell && typeof shell.summon === "function") shell.summon(pluginId, "{}")
  }
  function hideApproval() {
    if (shell && typeof shell.hide === "function") shell.hide(pluginId)
  }

  function copy(text, what) {
    Quickshell.execDetached(["wl-copy", "--", text])
    message((what || "Copied") + " to the clipboard", false)
  }

  function notify(headline, body) {
    Quickshell.execDetached(["omarchy-notification-send", "--app-name", "Opal", "-g", "󰌾", headline, body || ""])
  }

  function handle(msg) {
    if (msg.id !== undefined && msg.id !== null) {
      var cb = _callbacks[msg.id]
      if (cb) {
        delete _callbacks[msg.id]
        cb(msg.error || null, msg.result)
      }
      return
    }
    switch (msg.event) {
    case "state":
      status = msg.data || {}
      if (!locked) unlockRequest = null
      notifyDebounce.restart()
      statusDebounce.restart()
      break
    case "signer":
      var d = msg.data || {}
      if (d.type === "unlock_needed") {
        unlockRequest = d
        showApproval()
      } else if (d.type === "request") {
        activityDebounce.restart()
      } else {
        refreshApps()
      }
      break
    case "prompt":
      refreshPrompts()
      if (msg.data && msg.data.type === "opened") showApproval()
      break
    case "status":
      statusDebounce.restart()
      break
    case "notify":
      notifyDebounce.restart()
      break
    case "unread":
      var st = Object.assign({}, status)
      st.unread_notifications = (msg.data && msg.data.count) || 0
      status = st
      notifyDebounce.restart()
      break
    case "nostrconnect_offer":
      refreshOffers()
      showApproval()
      break
    }
  }

  Timer {
    id: statusDebounce
    interval: 500
    onTriggered: root.refreshStatus()
  }

  Timer {
    id: notifyDebounce
    interval: 600
    onTriggered: root.refreshNotifications()
  }

  Timer {
    id: activityDebounce
    interval: 400
    onTriggered: { root.refreshActivity(); root.refreshApps() }
  }

  Socket {
    id: sock
    path: root.socketPath
    parser: SplitParser {
      onRead: function(line) {
        var msg
        try { msg = JSON.parse(line) } catch (e) { return }
        root.handle(msg)
      }
    }
    onConnectedChanged: {
      if (connected) {
        root.everConnected = true
        root._callbacks = ({})
        root.call("subscribe", null, function(err, s) { if (!err) root.status = s || {} })
        root.refreshAll()
      } else {
        root.status = ({})
        reconnect.restart()
      }
    }
    onError: reconnect.restart()
  }

  // The daemon may start after the shell, or restart; keep trying.
  Timer {
    id: reconnect
    interval: 2000
    running: true
    onTriggered: {
      if (sock.connected) return
      sock.connected = false
      sock.connected = true
      if (!sock.connected) restart()
    }
  }

  // Keep "last used" times and stats fresh while idle.
  Timer {
    interval: 60000
    running: sock.connected
    repeat: true
    onTriggered: root.refreshActivity()
  }

  // `omarchy-shell opal <fn>` from scripts and keybindings.
  IpcHandler {
    target: "opal"
    function panel(): string { root.panelToggleRequested(); return "ok" }
    function tab(name: string): string { root.tabRequested(name); return "ok" }
    function notifications(): string { root.tabRequested("notifications"); root.panelToggleRequested(); return "ok" }
    function lock(): string { root.call("lock", null); return "ok" }
    function approvals(): string { root.showApproval(); return "ok" }
    function pending(): string { return String(root.attentionCount) }
  }
}
