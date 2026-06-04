using System.Net;
using System.Net.Sockets;
using Microsoft.AspNetCore.Http.Features;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

var builder = WebApplication.CreateBuilder(args);
var controlPlaneOptions = ControlPlaneOptions.Load();

builder.Services.ConfigureHttpJsonOptions(options =>
{
    options.SerializerOptions.WriteIndented = true;
});
builder.Services.AddSingleton<ControlPlaneState>();
builder.Services.AddSingleton(controlPlaneOptions);
builder.Services.AddSingleton<IPaymentProvider>(_ => new PaymentProviderAdapter(controlPlaneOptions));
builder.WebHost.ConfigureKestrel(options =>
{
    options.Limits.MaxRequestBodySize = controlPlaneOptions.MaxRequestBodyBytes;
});

var app = builder.Build();
LoadStateSnapshot(app.Services.GetRequiredService<ControlPlaneState>());
EnsureDemoUsers(
    app.Services.GetRequiredService<ControlPlaneState>(),
    app.Services.GetRequiredService<ControlPlaneOptions>());

app.Use(async (context, next) =>
{
    context.Response.Headers.TryAdd("X-Content-Type-Options", "nosniff");
    context.Response.Headers.TryAdd("X-Frame-Options", "DENY");
    context.Response.Headers.TryAdd("Referrer-Policy", "no-referrer");
    context.Response.Headers.TryAdd("Permissions-Policy", "camera=(), microphone=(), geolocation=()");

    if (context.Request.Path.StartsWithSegments("/api"))
    {
        context.Response.Headers.TryAdd("Cache-Control", "no-store");
    }

    await next();
});

app.Use(async (context, next) =>
{
    var options = context.RequestServices.GetRequiredService<ControlPlaneOptions>();
    var maxRequestBodySizeFeature = context.Features.Get<IHttpMaxRequestBodySizeFeature>();
    if (maxRequestBodySizeFeature is { IsReadOnly: false })
    {
        maxRequestBodySizeFeature.MaxRequestBodySize = options.MaxRequestBodyBytes;
    }

    if (context.Request.ContentLength is > 0 &&
        context.Request.ContentLength > options.MaxRequestBodyBytes)
    {
        context.Response.StatusCode = StatusCodes.Status413PayloadTooLarge;
        await context.Response.WriteAsJsonAsync(new ApiError("request_too_large", $"Request body exceeds {options.MaxRequestBodyBytes} bytes."));
        return;
    }

    await next();
});

app.Use(async (context, next) =>
{
    await next();
    if (HttpMethods.IsGet(context.Request.Method) ||
        context.Response.StatusCode >= StatusCodes.Status400BadRequest)
    {
        return;
    }

    var state = context.RequestServices.GetRequiredService<ControlPlaneState>();
    lock (state.SyncRoot)
    {
        SaveStateSnapshot(state);
    }
});

app.MapGet("/", () => TypedResults.Redirect("/api/health"));
app.MapGet("/admin", GetAdminDashboard);
app.MapGet("/api/health", GetHealth);
app.MapGet("/api/ready", GetReady);
app.MapGet("/api/config/runtime", GetRuntimeConfig);
app.MapGet("/api/admin/summary", GetAdminSummary);
app.MapGet("/api/admin/sessions", GetAdminSessions);
app.MapGet("/api/admin/billing/summary", GetAdminBillingSummary);
app.MapGet("/api/admin/billing/accounts", GetAdminBillingAccounts);
app.MapGet("/api/admin/billing/ledger", GetAdminBillingLedger);
app.MapGet("/api/admin/billing/provider", GetAdminBillingProvider);
app.MapGet("/api/admin/billing/reconciliation", GetAdminBillingReconciliation);
app.MapPost("/api/admin/hosts/{hostId}/offer", SetAdminHostOffer);
app.MapPost("/api/admin/hosts/{hostId}/availability", SetAdminHostAvailability);
app.MapPost("/api/admin/relays/{relayId}/availability", SetAdminRelayAvailability);
app.MapPost("/api/admin/sessions/{sessionId}/stop", StopAdminSession);
app.MapPost("/api/admin/billing/sessions/{sessionId}/settle", SettleAdminBillingSession);
app.MapPost("/api/admin/billing/sessions/{sessionId}/retry", RetryAdminBillingSession);
app.MapPost("/api/auth/device-login", DeviceLogin);
app.MapPost("/api/auth/refresh", RefreshAccessToken);
app.MapGet("/api/auth/me", GetCurrentDevice);
app.MapPost("/api/auth/users/register", RegisterUser);
app.MapPost("/api/auth/users/login", LoginUser);
app.MapPost("/api/auth/users/refresh", RefreshUserAccessToken);
app.MapGet("/api/auth/users/me", GetCurrentUser);
app.MapGet("/api/relay", GetRelays);
app.MapGet("/api/marketplace/hosts", GetMarketplaceHosts);
app.MapGet("/api/billing/sessions/{sessionId}", GetBillingSession);
app.MapPost("/api/relay/register", RegisterRelay);
app.MapPost("/api/relay/{relayId}/heartbeat", PostRelayHeartbeat);
app.MapPost("/api/hosts/register", RegisterHost);
app.MapPost("/api/hosts/{hostId}/heartbeat", PostHostHeartbeat);
app.MapGet("/api/hosts", GetHosts);
app.MapGet("/api/hosts/{hostId}", GetHostDetails);
app.MapGet("/api/hosts/{hostId}/lease", GetHostLease);
app.MapPost("/api/sessions", CreateSession);
app.MapGet("/api/sessions/{sessionId}", GetSession);
app.MapGet("/api/sessions/{sessionId}/connect", GetSessionConnectInstructions);
app.MapPost("/api/sessions/{sessionId}/nat/probe", PostSessionNatProbe);
app.MapPost("/api/sessions/{sessionId}/activate", ActivateSession);
app.MapPost("/api/sessions/{sessionId}/keepalive", KeepAliveSession);
app.MapPost("/api/sessions/{sessionId}/relay/register", PostSessionRelayRegistration);
app.MapPost("/api/sessions/{sessionId}/route/fallback", FallbackSessionRoute);
app.MapPost("/api/sessions/{sessionId}/route/recover", RecoverSessionRoute);
app.MapGet("/api/sessions/{sessionId}/route/policy", GetSessionRoutePolicy);
app.MapPost("/api/sessions/{sessionId}/stop", StopSession);
app.MapPost("/api/telemetry/session", IngestTelemetry);

app.Run();

static IResult GetAdminDashboard()
{
    const string html = """
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Everty Operator Console</title>
  <style>
    :root {
      --bg: #0d1117;
      --panel: rgba(22, 27, 34, 0.92);
      --panel-strong: #161b22;
      --line: rgba(139, 148, 158, 0.28);
      --text: #f0f6fc;
      --muted: #8b949e;
      --ok: #3fb950;
      --warn: #d29922;
      --bad: #f85149;
      --accent: #58a6ff;
      --accent-strong: #1f6feb;
      --shadow: 0 24px 80px rgba(0, 0, 0, 0.45);
      font-family: "Segoe UI", "Aptos", sans-serif;
    }

    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-height: 100vh;
      color: var(--text);
      background:
        radial-gradient(circle at top left, rgba(88, 166, 255, 0.24), transparent 32rem),
        radial-gradient(circle at bottom right, rgba(63, 185, 80, 0.14), transparent 28rem),
        var(--bg);
    }

    main {
      width: min(1180px, calc(100vw - 32px));
      margin: 0 auto;
      padding: 36px 0 48px;
    }

    header {
      display: grid;
      grid-template-columns: 1fr auto;
      gap: 24px;
      align-items: end;
      margin-bottom: 24px;
    }

    h1 {
      margin: 0;
      font-size: clamp(2rem, 5vw, 4.5rem);
      letter-spacing: -0.06em;
      line-height: 0.9;
    }

    .subtitle {
      margin: 14px 0 0;
      color: var(--muted);
      max-width: 680px;
      line-height: 1.6;
    }

    .toolbar, .panel {
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 24px;
      box-shadow: var(--shadow);
    }

    .toolbar {
      display: grid;
      grid-template-columns: minmax(220px, 1fr) auto auto;
      gap: 12px;
      padding: 14px;
      align-items: center;
    }

    input, select, button {
      border-radius: 14px;
      border: 1px solid var(--line);
      background: #0d1117;
      color: var(--text);
      padding: 12px 14px;
      font: inherit;
    }

    button {
      cursor: pointer;
      border-color: rgba(88, 166, 255, 0.45);
      background: linear-gradient(180deg, var(--accent), var(--accent-strong));
      color: white;
      font-weight: 700;
      transition: transform 120ms ease, filter 120ms ease;
    }

    button:hover { transform: translateY(-1px); filter: brightness(1.08); }
    button.secondary { background: #21262d; border-color: var(--line); }
    button.danger { background: linear-gradient(180deg, #ff7b72, #da3633); border-color: rgba(248, 81, 73, 0.7); }

    .grid {
      display: grid;
      grid-template-columns: repeat(4, minmax(0, 1fr));
      gap: 14px;
      margin: 18px 0;
    }

    .card {
      background: var(--panel-strong);
      border: 1px solid var(--line);
      border-radius: 22px;
      padding: 18px;
    }

    .metric {
      font-size: 2rem;
      font-weight: 800;
      letter-spacing: -0.04em;
    }

    .label {
      color: var(--muted);
      margin-top: 6px;
      font-size: 0.9rem;
    }

    .panel {
      padding: 18px;
      overflow: hidden;
    }

    .panel-head {
      display: flex;
      justify-content: space-between;
      align-items: center;
      gap: 16px;
      margin-bottom: 14px;
    }

    table {
      width: 100%;
      border-collapse: collapse;
      overflow: hidden;
    }

    th, td {
      padding: 12px 10px;
      text-align: left;
      border-bottom: 1px solid var(--line);
      vertical-align: top;
    }

    th {
      color: var(--muted);
      font-size: 0.78rem;
      text-transform: uppercase;
      letter-spacing: 0.12em;
    }

    .pill {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      border: 1px solid var(--line);
      border-radius: 999px;
      padding: 4px 9px;
      color: var(--muted);
      white-space: nowrap;
    }

    .status {
      min-height: 24px;
      color: var(--muted);
      font-size: 0.92rem;
    }

    .actions {
      display: flex;
      gap: 8px;
      flex-wrap: wrap;
    }

    .offer-form {
      display: grid;
      grid-template-columns: minmax(220px, 1.4fr) minmax(120px, 0.6fr) minmax(90px, 0.4fr) minmax(220px, 1.4fr) auto;
      gap: 10px;
      align-items: center;
      margin-top: 18px;
    }

    .check {
      display: inline-flex;
      gap: 8px;
      align-items: center;
      color: var(--muted);
    }

    .check input { width: 18px; height: 18px; }

    @media (max-width: 900px) {
      header, .toolbar, .grid, .offer-form { grid-template-columns: 1fr; }
      table, thead, tbody, th, td, tr { display: block; }
      thead { display: none; }
      tr { border-bottom: 1px solid var(--line); padding: 10px 0; }
      td { border: 0; padding: 8px 0; }
      td::before { content: attr(data-label); display: block; color: var(--muted); font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.12em; margin-bottom: 4px; }
    }
  </style>
</head>
<body>
  <main>
    <header>
      <div>
        <div class="pill">Everty Control Plane</div>
        <h1>Operator Console</h1>
        <p class="subtitle">Live diagnostics and guarded operator actions for local/product-phase deployments. Enter the operator key, refresh state, then stop sessions or disable hosts/relays.</p>
      </div>
      <div class="status" id="status">Waiting for operator key.</div>
    </header>

    <section class="toolbar">
      <input id="operatorKey" type="password" placeholder="EVERTY_CONTROL_PLANE_OPERATOR_KEY">
      <button id="refresh">Refresh</button>
      <button class="secondary" id="clear">Clear key</button>
    </section>

    <section class="grid" id="summaryGrid"></section>

    <section class="panel">
      <div class="panel-head">
        <div>
          <div class="pill">Marketplace Offers</div>
          <div class="label">Publish or unlist host pricing for the first marketplace/billing skeleton.</div>
        </div>
      </div>
      <div class="offer-form">
        <input id="offerHostId" placeholder="hostId">
        <input id="offerPrice" type="number" min="0" step="0.01" placeholder="price / hour">
        <input id="offerCurrency" maxlength="8" placeholder="USD" value="USD">
        <input id="offerDescription" maxlength="240" placeholder="description">
        <label class="check"><input id="offerListed" type="checkbox" checked> listed</label>
      </div>
      <div class="actions" style="margin-top:12px">
        <button id="saveOffer">Save offer</button>
      </div>
    </section>

    <section class="panel">
      <div class="panel-head">
        <div>
          <div class="pill">Managed Sessions</div>
          <div class="label">Stop stale or broken sessions from the operator channel.</div>
        </div>
        <button class="secondary" id="refreshSessions">Reload sessions</button>
      </div>
      <div style="overflow:auto">
        <table>
          <thead>
            <tr>
              <th>Session</th>
              <th>Host</th>
              <th>Status</th>
              <th>Route</th>
              <th>Billing</th>
              <th>Actor</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody id="sessions"></tbody>
        </table>
      </div>
    </section>

    <section class="panel">
      <div class="panel-head">
        <div>
          <div class="pill">Billing Reconciliation</div>
          <div class="label">Retry failed capture/settle operations after a provider outage or inspect sessions that need payout action.</div>
        </div>
        <button class="secondary" id="refreshReconciliation">Reload reconciliation</button>
      </div>
      <div style="overflow:auto">
        <table>
          <thead>
            <tr>
              <th>Session</th>
              <th>Status</th>
              <th>Provider</th>
              <th>Amounts</th>
              <th>Error</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody id="reconciliation"></tbody>
        </table>
      </div>
    </section>
  </main>
  <script>
    const keyInput = document.getElementById("operatorKey");
    const statusEl = document.getElementById("status");
    const summaryGrid = document.getElementById("summaryGrid");
    const sessionsBody = document.getElementById("sessions");
    const reconciliationBody = document.getElementById("reconciliation");
    const offerHostId = document.getElementById("offerHostId");
    const offerPrice = document.getElementById("offerPrice");
    const offerCurrency = document.getElementById("offerCurrency");
    const offerDescription = document.getElementById("offerDescription");
    const offerListed = document.getElementById("offerListed");

    keyInput.value = localStorage.getItem("evertyOperatorKey") || "";

    function setStatus(text, kind = "muted") {
      statusEl.textContent = text;
      statusEl.style.color = kind === "bad" ? "var(--bad)" : kind === "ok" ? "var(--ok)" : "var(--muted)";
    }

    function key() {
      const value = keyInput.value.trim();
      if (value) localStorage.setItem("evertyOperatorKey", value);
      return value;
    }

    async function api(path, options = {}) {
      const operatorKey = key();
      if (!operatorKey) throw new Error("Operator key is required.");
      const response = await fetch(path, {
        ...options,
        headers: {
          "X-Everty-Operator-Key": operatorKey,
          "Content-Type": "application/json",
          ...(options.headers || {})
        }
      });
      if (!response.ok) {
        const text = await response.text();
        throw new Error(`${response.status} ${response.statusText}: ${text}`);
      }
      return response.json();
    }

    function metric(label, value) {
      return `<div class="card"><div class="metric">${value}</div><div class="label">${label}</div></div>`;
    }

    async function loadSummary() {
      const summary = await api("/api/admin/summary");
      const billing = await api("/api/admin/billing/summary");
      const reconciliation = await api("/api/admin/billing/reconciliation");
      summaryGrid.innerHTML = [
        metric("online hosts", `${summary.onlineHosts}/${summary.registeredHosts}`),
        metric("online relays", `${summary.onlineRelays}/${summary.registeredRelays}`),
        metric("active sessions", summary.activeSessions),
        metric("billing holds", `${billing.pendingHolds}/${billing.totalHolds}`),
        metric("billing action", reconciliation.length)
      ].join("");
    }

    async function loadSessions() {
      const sessions = await api("/api/admin/sessions");
      sessionsBody.innerHTML = sessions.map(session => `
        <tr>
          <td data-label="Session"><code>${session.sessionId}</code><div class="label">${new Date(session.updatedUtc).toLocaleString()}</div></td>
          <td data-label="Host"><code>${session.hostId}</code><div class="label">${session.clientLabel || ""}</div></td>
          <td data-label="Status"><span class="pill">${session.status}</span></td>
          <td data-label="Route"><span class="pill">${session.routeKind}</span><div class="label">${session.routeState} / v${session.routeVersion}</div></td>
          <td data-label="Billing"><span class="pill">${session.billingStatus}</span><div class="label">${session.billingHoldAmount} ${session.billingCurrency} hold / ${session.billingSettledAmount} settled</div></td>
          <td data-label="Actor">${session.createdByActor}</td>
          <td data-label="Actions"><div class="actions">
            <button class="danger" data-stop="${session.sessionId}">Stop</button>
            <button class="secondary" data-settle="${session.sessionId}">Settle billing</button>
            <button class="secondary" data-host="${session.hostId}">Disable host</button>
            ${session.relayId ? `<button class="secondary" data-relay="${session.relayId}">Disable relay</button>` : ""}
          </div></td>
        </tr>`).join("") || `<tr><td colspan="7">No sessions.</td></tr>`;
    }

    async function loadReconciliation() {
      const items = await api("/api/admin/billing/reconciliation");
      reconciliationBody.innerHTML = items.map(item => `
        <tr>
          <td data-label="Session"><code>${item.sessionId}</code><div class="label">${new Date(item.updatedUtc).toLocaleString()}</div></td>
          <td data-label="Status"><span class="pill">${item.billingStatus}</span><div class="label">session ${item.sessionStatus ?? "missing"} / action ${item.actionRequired}</div></td>
          <td data-label="Provider"><span class="pill">${item.paymentProvider}</span><div class="label">${item.providerCaptureId || item.providerHoldId || "no provider id"}</div></td>
          <td data-label="Amounts">${item.capturedAmount} / ${item.settledAmount} ${item.currency}<div class="label">${item.holdAmount} hold</div></td>
          <td data-label="Error">${item.lastPaymentError || "-"}<div class="label">${item.lastPaymentAttemptUtc ? new Date(item.lastPaymentAttemptUtc).toLocaleString() : "no attempt"}</div></td>
          <td data-label="Actions"><div class="actions">
            <button class="secondary" data-retry="${item.sessionId}" data-action="${item.actionRequired}">Retry ${item.actionRequired}</button>
          </div></td>
        </tr>`).join("") || `<tr><td colspan="6">No billing reconciliation actions.</td></tr>`;
    }

    async function refreshAll() {
      try {
        setStatus("Refreshing...");
        await loadSummary();
        await loadSessions();
        await loadReconciliation();
        setStatus("Updated.", "ok");
      } catch (error) {
        setStatus(error.message, "bad");
      }
    }

    async function postJson(path, body) {
      return api(path, { method: "POST", body: JSON.stringify(body) });
    }

    document.getElementById("refresh").addEventListener("click", refreshAll);
    document.getElementById("refreshSessions").addEventListener("click", refreshAll);
    document.getElementById("refreshReconciliation").addEventListener("click", refreshAll);
    document.getElementById("clear").addEventListener("click", () => {
      localStorage.removeItem("evertyOperatorKey");
      keyInput.value = "";
      setStatus("Operator key cleared.");
    });
    document.getElementById("saveOffer").addEventListener("click", async () => {
      try {
        const hostId = offerHostId.value.trim();
        if (!hostId) throw new Error("hostId is required.");
        await postJson(`/api/admin/hosts/${hostId}/offer`, {
          listed: offerListed.checked,
          pricePerHour: Number(offerPrice.value || "0"),
          currency: offerCurrency.value || "USD",
          description: offerDescription.value || null
        });
        setStatus("Offer saved.", "ok");
        await loadSummary();
      } catch (error) {
        setStatus(error.message, "bad");
      }
    });

    sessionsBody.addEventListener("click", async (event) => {
      const target = event.target;
      if (!(target instanceof HTMLButtonElement)) return;
      try {
        if (target.dataset.stop) {
          await postJson(`/api/admin/sessions/${target.dataset.stop}/stop`, { reason: "operator_console_stop" });
        }
        if (target.dataset.settle) {
          await postJson(`/api/admin/billing/sessions/${target.dataset.settle}/settle`, { reason: "operator_console_settle" });
        }
        if (target.dataset.host) {
          await postJson(`/api/admin/hosts/${target.dataset.host}/availability`, { availability: "Disabled", reason: "operator_console_disable_host", stopActiveSession: true });
        }
        if (target.dataset.relay) {
          await postJson(`/api/admin/relays/${target.dataset.relay}/availability`, { availability: "Disabled", reason: "operator_console_disable_relay" });
        }
        await refreshAll();
      } catch (error) {
        setStatus(error.message, "bad");
      }
    });

    reconciliationBody.addEventListener("click", async (event) => {
      const target = event.target;
      if (!(target instanceof HTMLButtonElement)) return;
      try {
        if (target.dataset.retry) {
          await postJson(`/api/admin/billing/sessions/${target.dataset.retry}/retry`, {
            action: target.dataset.action || "auto",
            reason: "operator_console_retry"
          });
        }
        await refreshAll();
      } catch (error) {
        setStatus(error.message, "bad");
      }
    });

    if (keyInput.value) refreshAll();
  </script>
</body>
</html>
""";

    return Results.Content(html, "text/html");
}

static IResult GetHealth(ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        var onlineHosts = state.Hosts.Values.Count(host => IsHostOnline(host, now));
        return Results.Ok(new HealthResponse(
            Service: "everty-control-plane",
            BuildMarker: GetBuildMarker(),
            UtcNow: now,
            RegisteredHosts: state.Hosts.Count,
            OnlineHosts: onlineHosts,
            ActiveSessions: state.Sessions.Values.Count(session => session.Status is SessionStatus.Pending or SessionStatus.Active),
            TelemetryEvents: state.Telemetry.Count));
    }
}

static IResult GetReady(ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    var persistence = CheckPersistenceReadiness();
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        var response = new ReadyResponse(
            Service: "everty-control-plane",
            BuildMarker: GetBuildMarker(),
            UtcNow: now,
            Ready: persistence.Writable,
            PersistencePath: persistence.Path,
            PersistenceWritable: persistence.Writable,
            PersistenceError: persistence.Error,
            RegisteredHosts: state.Hosts.Count,
            ActiveSessions: state.Sessions.Values.Count(session => session.Status is SessionStatus.Pending or SessionStatus.Active));
        return persistence.Writable
            ? Results.Ok(response)
            : Results.Problem(
                title: "Control plane is not ready.",
                detail: persistence.Error ?? "Persistence path is not writable.",
                statusCode: StatusCodes.Status503ServiceUnavailable,
                extensions: new Dictionary<string, object?> { ["ready"] = response });
    }
}

static IResult GetRuntimeConfig(ControlPlaneOptions options, IPaymentProvider paymentProvider)
{
    return Results.Ok(new RuntimeConfigResponse(
        Service: "everty-control-plane",
        BuildMarker: GetBuildMarker(),
        AccessTokenHours: options.AccessTokenLifetime.Hours + (options.AccessTokenLifetime.Days * 24),
        RefreshTokenDays: options.RefreshTokenLifetime.Days,
        MaxRequestBodyBytes: options.MaxRequestBodyBytes,
        OperatorAuthConfigured: options.OperatorAuthConfigured,
        DemoAuthEnabled: options.DemoAuthEnabled,
        PaymentProvider: paymentProvider.Provider,
        PaymentProviderMode: paymentProvider.Mode,
        PaymentProviderEndpointConfigured: paymentProvider.EndpointConfigured,
        SecurityHeadersEnabled: true,
        PersistencePath: GetStateSnapshotPath()));
}

static IResult GetAdminSummary(HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        return Results.Ok(new AdminSummaryResponse(
            Service: "everty-control-plane",
            UtcNow: now,
            RegisteredHosts: state.Hosts.Count,
            OnlineHosts: state.Hosts.Values.Count(host => IsHostOnline(host, now)),
            RegisteredRelays: state.Relays.Count,
            OnlineRelays: state.Relays.Values.Count(relay => IsRelayOnline(relay, now)),
            Sessions: state.Sessions.Count,
            ActiveSessions: state.Sessions.Values.Count(session => session.Status is SessionStatus.Pending or SessionStatus.Active),
            TelemetryEvents: state.Telemetry.Count,
            MarketplaceOffers: state.HostOffers.Count,
            ListedMarketplaceOffers: state.HostOffers.Values.Count(offer => offer.Listed),
            PersistencePath: GetStateSnapshotPath(),
            OperatorAuthConfigured: options.OperatorAuthConfigured));
    }
}

static IResult GetAdminSessions(HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        var sessions = state.Sessions.Values
            .OrderByDescending(session => session.UpdatedUtc)
            .Select(session =>
            {
                state.BillingSessions.TryGetValue(session.SessionId, out var billing);
                return new AdminSessionSummary(
                SessionId: session.SessionId,
                HostId: session.HostId,
                ClientLabel: session.ClientLabel,
                ClientRegion: session.ClientRegion,
                Status: session.Status,
                RouteKind: session.RouteKind,
                RouteState: ComputeRouteState(session.RouteKind, session.Status),
                RouteVersion: session.RouteVersion,
                RelayId: session.RelayId,
                CreatedByActor: DescribeSessionCreator(session),
                CreatedUtc: session.CreatedUtc,
                UpdatedUtc: session.UpdatedUtc,
                ExpiresUtc: session.ExpiresUtc,
                StopReason: session.StopReason,
                BillingStatus: billing?.Status ?? BillingStatus.None,
                BillingHoldAmount: billing?.HoldAmount ?? 0m,
                BillingCapturedAmount: billing?.CapturedAmount ?? 0m,
                BillingSettledAmount: billing?.SettledAmount ?? 0m,
                BillingCurrency: billing?.Currency ?? GetHostBillingCurrency(state, session.HostId));
            })
            .ToArray();
        return Results.Ok(sessions);
    }
}

static IResult GetAdminBillingSummary(HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        var summary = BuildBillingSummary(state, now);
        return Results.Ok(summary);
    }
}

static IResult GetAdminBillingProvider(HttpRequest httpRequest, ControlPlaneOptions options, IPaymentProvider paymentProvider)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    return Results.Ok(new BillingProviderResponse(
        Provider: paymentProvider.Provider,
        Mode: paymentProvider.Mode,
        Configured: paymentProvider.Configured,
        EndpointConfigured: paymentProvider.EndpointConfigured,
        ExternalCallsEnabled: paymentProvider.ExternalCallsEnabled,
        ManualCapture: paymentProvider.ManualCapture,
        ManualSettlement: paymentProvider.ManualSettlement));
}

static IResult GetAdminBillingAccounts(HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        var accounts = state.BillingAccounts.Values
            .OrderBy(account => account.HostId, StringComparer.OrdinalIgnoreCase)
            .Select(account => new BillingAccountSummary(
                HostId: account.HostId,
                Currency: account.Currency,
                Balance: account.Balance,
                PendingAmount: account.PendingAmount,
                PlatformCommissionRate: account.PlatformCommissionRate,
                UpdatedUtc: account.UpdatedUtc))
            .ToArray();
        return Results.Ok(accounts);
    }
}

static IResult GetAdminBillingLedger(HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options, int limit = 100)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        var normalizedLimit = Math.Clamp(limit <= 0 ? 100 : limit, 1, 500);
        var entries = state.BillingLedger
            .OrderByDescending(entry => entry.RecordedUtc)
            .Take(normalizedLimit)
            .ToArray();
        return Results.Ok(entries);
    }
}

static IResult GetAdminBillingReconciliation(HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        var items = state.BillingSessions.Values
            .Select(billing => ToBillingReconciliationItem(billing, state, now))
            .Where(item => item.ActionRequired is not "none")
            .OrderByDescending(item => item.UpdatedUtc)
            .ToArray();
        return Results.Ok(items);
    }
}

static IResult SetAdminHostOffer(string hostId, AdminHostOfferRequest request, HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    if (request.PricePerHour < 0 || request.PricePerHour > 100_000)
    {
        return Results.BadRequest(new ApiError("offer_price_invalid", "PricePerHour must be between 0 and 100000."));
    }

    var currency = NormalizeCurrency(request.Currency);
    var description = NormalizeDescription(request.Description);
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Hosts.TryGetValue(hostId, out var host))
        {
            return Results.NotFound(new ApiError("host_not_found", $"Host '{hostId}' was not found."));
        }

        var createdUtc = state.HostOffers.TryGetValue(hostId, out var existing)
            ? existing.CreatedUtc
            : now;
        var offer = new HostOfferRecord(
            HostId: hostId,
            Listed: request.Listed,
            PricePerHour: Math.Round(request.PricePerHour, 2, MidpointRounding.AwayFromZero),
            Currency: currency,
            Description: description,
            CreatedUtc: createdUtc,
            UpdatedUtc: now);
        state.HostOffers[hostId] = offer;

        return Results.Ok(ToMarketplaceHostOffer(host, offer, now));
    }
}

static IResult SettleAdminBillingSession(string sessionId, AdminBillingSettleRequest request, HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options, IPaymentProvider paymentProvider)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!state.Hosts.TryGetValue(session.HostId, out var host))
        {
            return Results.NotFound(new ApiError("host_not_found", $"Host '{session.HostId}' was not found."));
        }

        if (!state.BillingSessions.TryGetValue(sessionId, out var billing))
        {
            return Results.NotFound(new ApiError("billing_session_not_found", $"Billing session '{sessionId}' was not found."));
        }

        var settled = SettleBillingSession(state, session, host, billing, options, paymentProvider, now, Normalize(request.Reason, "operator_settle"));
        return Results.Ok(settled);
    }
}

static IResult RetryAdminBillingSession(string sessionId, AdminBillingRetryRequest request, HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options, IPaymentProvider paymentProvider)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!state.Hosts.TryGetValue(session.HostId, out var host))
        {
            return Results.NotFound(new ApiError("host_not_found", $"Host '{session.HostId}' was not found."));
        }

        if (!state.BillingSessions.TryGetValue(sessionId, out var billing))
        {
            return Results.NotFound(new ApiError("billing_session_not_found", $"Billing session '{sessionId}' was not found."));
        }

        var action = Normalize(request.Action, "auto").ToLowerInvariant();
        var reason = Normalize(request.Reason, "operator_retry");
        if (action is "auto")
        {
            action = ResolveBillingReconciliationAction(session, billing);
        }

        if (action is "capture")
        {
            if (session.Status is not SessionStatus.Stopped and not SessionStatus.Expired)
            {
                return Results.Conflict(new ApiError("billing_capture_not_ready", "Billing capture can only be retried after the session is stopped or expired."));
            }

            CaptureBillingForSession(state, session, host, options, paymentProvider, now, reason);
            return Results.Ok(ToBillingSessionDetails(session, state.BillingSessions[sessionId], state, now));
        }

        if (action is "settle")
        {
            var settled = SettleBillingSession(state, session, host, billing, options, paymentProvider, now, reason);
            return Results.Ok(settled);
        }

        return Results.BadRequest(new ApiError("billing_retry_action_invalid", "Retry action must be auto, capture, or settle."));
    }
}

static IResult SetAdminHostAvailability(string hostId, AdminHostAvailabilityRequest request, HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    if (!Enum.TryParse<HostAvailability>(Normalize(request.Availability, string.Empty), ignoreCase: true, out var requestedAvailability))
    {
        return Results.BadRequest(new ApiError("host_availability_invalid", "Availability must be Offline, Online, Busy, or Disabled."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Hosts.TryGetValue(hostId, out var host))
        {
            return Results.NotFound(new ApiError("host_not_found", $"Host '{hostId}' was not found."));
        }

        if (!string.IsNullOrWhiteSpace(host.ActiveSessionId) &&
            requestedAvailability is HostAvailability.Disabled or HostAvailability.Offline &&
            !request.StopActiveSession)
        {
            return Results.Conflict(new ApiError("host_has_active_session", "Set StopActiveSession=true to disable/offline a host with an active session."));
        }

        if (!string.IsNullOrWhiteSpace(host.ActiveSessionId) &&
            request.StopActiveSession &&
            state.Sessions.TryGetValue(host.ActiveSessionId, out var activeSession) &&
            activeSession.Status is SessionStatus.Pending or SessionStatus.Active)
        {
            state.Sessions[activeSession.SessionId] = activeSession with
            {
                Status = SessionStatus.Stopped,
                StopReason = Normalize(request.Reason, "operator_host_availability_change"),
                UpdatedUtc = now,
            };
        }

        var updated = host with
        {
            Availability = requestedAvailability,
            ActiveSessionId = requestedAvailability is HostAvailability.Disabled or HostAvailability.Offline ? null : host.ActiveSessionId,
            UpdatedUtc = now,
        };
        state.Hosts[hostId] = updated;
        return Results.Ok(ToHostSummary(updated, now));
    }
}

static IResult SetAdminRelayAvailability(string relayId, AdminRelayAvailabilityRequest request, HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    if (!Enum.TryParse<RelayAvailability>(Normalize(request.Availability, string.Empty), ignoreCase: true, out var requestedAvailability))
    {
        return Results.BadRequest(new ApiError("relay_availability_invalid", "Availability must be Offline, Online, or Disabled."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Relays.TryGetValue(relayId, out var relay))
        {
            return Results.NotFound(new ApiError("relay_not_found", $"Relay '{relayId}' was not found."));
        }

        var updated = relay with
        {
            Availability = requestedAvailability,
            UpdatedUtc = now,
        };
        state.Relays[relayId] = updated;
        return Results.Ok(ToRelaySummary(updated, state, now));
    }
}

static IResult StopAdminSession(string sessionId, AdminStopSessionRequest request, HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options, IPaymentProvider paymentProvider)
{
    if (!TryAuthorizeOperator(httpRequest, options, out var error))
    {
        return error!;
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        var updatedSession = session with
        {
            Status = SessionStatus.Stopped,
            StopReason = Normalize(request.Reason, "operator_stop"),
            UpdatedUtc = now,
        };
        state.Sessions[sessionId] = updatedSession;

        if (state.Hosts.TryGetValue(session.HostId, out var hostForBilling))
        {
            CaptureBillingForSession(state, updatedSession, hostForBilling, options, paymentProvider, now, Normalize(request.Reason, "operator_stop"));
        }

        if (state.Hosts.TryGetValue(session.HostId, out var host))
        {
            state.Hosts[session.HostId] = ReleaseHostAfterSessionStop(host, now);
        }

        var response = Results.Ok(ToSessionDetails(updatedSession, state.Hosts.GetValueOrDefault(updatedSession.HostId), state, now));
        state.Sessions.Remove(sessionId);
        return response;
    }
}

static IResult DeviceLogin(DeviceLoginRequest request, ControlPlaneState state, ControlPlaneOptions options)
{
    if (string.IsNullOrWhiteSpace(request.DeviceLabel))
    {
        return Results.BadRequest(new ApiError("device_label_required", "DeviceLabel is required."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);

        DeviceRecord device;
        if (!string.IsNullOrWhiteSpace(request.DeviceId) &&
            !string.IsNullOrWhiteSpace(request.DeviceSecret) &&
            state.Devices.TryGetValue(request.DeviceId, out var existingDevice) &&
            FixedTimeEquals(existingDevice.DeviceSecret, request.DeviceSecret))
        {
            device = existingDevice with
            {
                DeviceLabel = request.DeviceLabel.Trim(),
                Platform = Normalize(request.Platform, existingDevice.Platform),
                LastSeenUtc = now,
                UpdatedUtc = now,
            };
        }
        else
        {
            device = new DeviceRecord(
                DeviceId: $"device_{Guid.NewGuid():N}",
                DeviceSecret: CreateSecret(),
                DeviceLabel: request.DeviceLabel.Trim(),
                Platform: Normalize(request.Platform, "unknown"),
                CreatedUtc: now,
                UpdatedUtc: now,
                LastSeenUtc: now);
        }

        state.Devices[device.DeviceId] = device;

        RevokeDeviceTokens(state, device.DeviceId);
        var (accessToken, refreshToken) = IssueDeviceTokens(state, device.DeviceId, now, options);

        return Results.Ok(new DeviceLoginResponse(
            DeviceId: device.DeviceId,
            DeviceSecret: device.DeviceSecret,
            AccessToken: accessToken.AccessToken,
            ExpiresUtc: accessToken.ExpiresUtc,
            RefreshToken: refreshToken.RefreshToken,
            RefreshExpiresUtc: refreshToken.ExpiresUtc,
            Device: ToDeviceSummary(device)));
    }
}

static IResult RefreshAccessToken(RefreshAccessTokenRequest request, ControlPlaneState state, ControlPlaneOptions options)
{
    if (string.IsNullOrWhiteSpace(request.RefreshToken))
    {
        return Results.BadRequest(new ApiError("refresh_token_required", "RefreshToken is required."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.RefreshTokens.TryGetValue(request.RefreshToken.Trim(), out var refreshToken) || refreshToken.ExpiresUtc <= now)
        {
            return Results.Unauthorized();
        }

        if (!state.Devices.TryGetValue(refreshToken.DeviceId, out var device))
        {
            return Results.Unauthorized();
        }

        state.RefreshTokens.Remove(refreshToken.RefreshToken);
        foreach (var token in state.AccessTokens.Values.Where(token => string.Equals(token.DeviceId, device.DeviceId, StringComparison.Ordinal)).ToArray())
        {
            state.AccessTokens.Remove(token.AccessToken);
        }

        var (accessToken, rotatedRefreshToken) = IssueDeviceTokens(state, device.DeviceId, now, options);
        return Results.Ok(new RefreshAccessTokenResponse(
            AccessToken: accessToken.AccessToken,
            ExpiresUtc: accessToken.ExpiresUtc,
            RefreshToken: rotatedRefreshToken.RefreshToken,
            RefreshExpiresUtc: rotatedRefreshToken.ExpiresUtc,
            Device: ToDeviceSummary(device)));
    }
}

static IResult GetCurrentDevice(HttpRequest httpRequest, ControlPlaneState state)
{
    if (!TryAuthorizeClient(httpRequest, state, out var accessToken, out var device, out var error))
    {
        return error!;
    }

    return Results.Ok(new DeviceSessionResponse(
        Device: ToDeviceSummary(device!),
        ExpiresUtc: accessToken!.ExpiresUtc));
}

static IResult RegisterUser(RegisterUserRequest request, ControlPlaneState state, ControlPlaneOptions options)
{
    var normalizedEmail = NormalizeEmail(request.Email);
    if (string.IsNullOrWhiteSpace(normalizedEmail))
    {
        return Results.BadRequest(new ApiError("email_required", "Email is required."));
    }

    if (string.IsNullOrWhiteSpace(request.Password) || request.Password.Trim().Length < 4)
    {
        return Results.BadRequest(new ApiError("password_too_short", "Password must be at least 4 characters."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (state.Users.Values.Any(user => string.Equals(user.Email, normalizedEmail, StringComparison.OrdinalIgnoreCase)))
        {
            return Results.Conflict(new ApiError("user_exists", $"User '{normalizedEmail}' already exists."));
        }

        var salt = CreateSecret();
        var user = new UserRecord(
            UserId: $"user_{Guid.NewGuid():N}",
            Email: normalizedEmail,
            PasswordSalt: salt,
            PasswordHash: HashPassword(salt, request.Password),
            CreatedUtc: now,
            UpdatedUtc: now,
            LastSeenUtc: now,
            Enabled: true);

        state.Users[user.UserId] = user;
        var (accessToken, refreshToken) = IssueUserTokens(state, user.UserId, now, options);
        return Results.Ok(new UserLoginResponse(
            AccessToken: accessToken.AccessToken,
            ExpiresUtc: accessToken.ExpiresUtc,
            RefreshToken: refreshToken.RefreshToken,
            RefreshExpiresUtc: refreshToken.ExpiresUtc,
            User: ToUserSummary(user)));
    }
}

static IResult LoginUser(UserLoginRequest request, ControlPlaneState state, ControlPlaneOptions options)
{
    var normalizedEmail = NormalizeEmail(request.Email);
    if (string.IsNullOrWhiteSpace(normalizedEmail))
    {
        return Results.BadRequest(new ApiError("email_required", "Email is required."));
    }

    if (string.IsNullOrWhiteSpace(request.Password))
    {
        return Results.BadRequest(new ApiError("password_required", "Password is required."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        var user = state.Users.Values.FirstOrDefault(candidate => string.Equals(candidate.Email, normalizedEmail, StringComparison.OrdinalIgnoreCase));
        if (user is null || !user.Enabled || !FixedTimeEquals(user.PasswordHash, HashPassword(user.PasswordSalt, request.Password)))
        {
            return Results.Unauthorized();
        }

        user = user with
        {
            LastSeenUtc = now,
            UpdatedUtc = now,
        };
        state.Users[user.UserId] = user;

        RevokeUserTokens(state, user.UserId);
        var (accessToken, refreshToken) = IssueUserTokens(state, user.UserId, now, options);
        return Results.Ok(new UserLoginResponse(
            AccessToken: accessToken.AccessToken,
            ExpiresUtc: accessToken.ExpiresUtc,
            RefreshToken: refreshToken.RefreshToken,
            RefreshExpiresUtc: refreshToken.ExpiresUtc,
            User: ToUserSummary(user)));
    }
}

static IResult RefreshUserAccessToken(UserRefreshAccessTokenRequest request, ControlPlaneState state, ControlPlaneOptions options)
{
    if (string.IsNullOrWhiteSpace(request.RefreshToken))
    {
        return Results.BadRequest(new ApiError("refresh_token_required", "RefreshToken is required."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.UserRefreshTokens.TryGetValue(request.RefreshToken.Trim(), out var refreshToken) || refreshToken.ExpiresUtc <= now)
        {
            return Results.Unauthorized();
        }

        if (!state.Users.TryGetValue(refreshToken.UserId, out var user) || !user.Enabled)
        {
            return Results.Unauthorized();
        }

        state.UserRefreshTokens.Remove(refreshToken.RefreshToken);
        foreach (var token in state.UserAccessTokens.Values.Where(token => string.Equals(token.UserId, user.UserId, StringComparison.Ordinal)).ToArray())
        {
            state.UserAccessTokens.Remove(token.AccessToken);
        }

        user = user with
        {
            LastSeenUtc = now,
            UpdatedUtc = now,
        };
        state.Users[user.UserId] = user;

        var (accessToken, rotatedRefreshToken) = IssueUserTokens(state, user.UserId, now, options);
        return Results.Ok(new UserLoginResponse(
            AccessToken: accessToken.AccessToken,
            ExpiresUtc: accessToken.ExpiresUtc,
            RefreshToken: rotatedRefreshToken.RefreshToken,
            RefreshExpiresUtc: rotatedRefreshToken.ExpiresUtc,
            User: ToUserSummary(user)));
    }
}

static IResult GetCurrentUser(HttpRequest httpRequest, ControlPlaneState state)
{
    if (!TryAuthorizeUser(httpRequest, state, out var accessToken, out var user, out var error))
    {
        return error!;
    }

    return Results.Ok(new UserSessionResponse(
        User: ToUserSummary(user!),
        ExpiresUtc: accessToken!.ExpiresUtc));
}

static IResult GetRelays(HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!TryAuthorizeClientActor(httpRequest, state, out _, out var error))
        {
            return error!;
        }

        var relays = state.Relays.Values
            .OrderBy(relay => relay.DisplayName, StringComparer.OrdinalIgnoreCase)
            .Select(relay => ToRelaySummary(relay, state, now))
            .ToArray();
        return Results.Ok(relays);
    }
}

static IResult RegisterRelay(RegisterRelayRequest request, ControlPlaneState state)
{
    if (string.IsNullOrWhiteSpace(request.DisplayName))
    {
        return Results.BadRequest(new ApiError("display_name_required", "DisplayName is required."));
    }

    if (string.IsNullOrWhiteSpace(request.PublicAddress) || request.UdpPort is < 1 or > 65535)
    {
        return Results.BadRequest(new ApiError("relay_endpoint_required", "PublicAddress and a valid UdpPort are required."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        RelayRecord? existingRelay = null;
        var hasExistingRegistration =
            !string.IsNullOrWhiteSpace(request.RelayId) &&
            !string.IsNullOrWhiteSpace(request.RelaySecret) &&
            state.Relays.TryGetValue(request.RelayId, out existingRelay) &&
            FixedTimeEquals(existingRelay.RelaySecret, request.RelaySecret);

        RelayRecord relay;
        if (hasExistingRegistration)
        {
            relay = existingRelay! with
            {
                DisplayName = request.DisplayName.Trim(),
                Region = Normalize(request.Region, existingRelay!.Region),
                PublicAddress = request.PublicAddress.Trim(),
                UdpPort = request.UdpPort,
                Availability = request.Availability ?? RelayAvailability.Online,
                UpdatedUtc = now,
                LastSeenUtc = now,
            };
        }
        else
        {
            relay = new RelayRecord(
                RelayId: $"relay_{Guid.NewGuid():N}",
                RelaySecret: CreateSecret(),
                DisplayName: request.DisplayName.Trim(),
                Region: Normalize(request.Region, "global"),
                PublicAddress: request.PublicAddress.Trim(),
                UdpPort: request.UdpPort,
                Availability: request.Availability ?? RelayAvailability.Online,
                CreatedUtc: now,
                UpdatedUtc: now,
                LastSeenUtc: now);
        }

        state.Relays[relay.RelayId] = relay;
        return Results.Ok(new RegisterRelayResponse(
            RelayId: relay.RelayId,
            RelaySecret: relay.RelaySecret,
            HeartbeatIntervalSeconds: 5,
            Relay: ToRelaySummary(relay, state, now)));
    }
}

static IResult PostRelayHeartbeat(string relayId, RelayHeartbeatRequest request, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Relays.TryGetValue(relayId, out var relay))
        {
            return Results.NotFound(new ApiError("relay_not_found", $"Relay '{relayId}' was not found."));
        }

        if (!FixedTimeEquals(relay.RelaySecret, request.RelaySecret))
        {
            return Results.Unauthorized();
        }

        var updated = relay with
        {
            Availability = request.Availability ?? RelayAvailability.Online,
            PublicAddress = Normalize(request.PublicAddress, relay.PublicAddress),
            UdpPort = request.UdpPort is > 0 and <= 65535 ? request.UdpPort : relay.UdpPort,
            UpdatedUtc = now,
            LastSeenUtc = now,
        };

        state.Relays[relayId] = updated;
        return Results.Ok(new RelayHeartbeatResponse(
            RelayId: relayId,
            Availability: updated.Availability,
            Online: IsRelayOnline(updated, now),
            ServerUtc: now));
    }
}

static IResult RegisterHost(RegisterHostRequest request, ControlPlaneState state)
{
    if (string.IsNullOrWhiteSpace(request.DisplayName))
    {
        return Results.BadRequest(new ApiError("display_name_required", "DisplayName is required."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        HostRecord? existingHost = null;
        var hasExistingRegistration =
            !string.IsNullOrWhiteSpace(request.HostId) &&
            !string.IsNullOrWhiteSpace(request.HostSecret) &&
            state.Hosts.TryGetValue(request.HostId, out existingHost) &&
            FixedTimeEquals(existingHost.HostSecret, request.HostSecret);

        HostRecord host;
        if (hasExistingRegistration)
        {
            host = existingHost! with
            {
                DisplayName = request.DisplayName.Trim(),
                Region = Normalize(request.Region, "global"),
                DirectAddress = Normalize(request.DirectAddress, existingHost!.DirectAddress),
                DirectPort = request.DirectPort > 0 ? request.DirectPort : existingHost!.DirectPort,
                EncoderBackends = NormalizeList(request.EncoderBackends, existingHost!.EncoderBackends),
                SupportsHevc = request.SupportsHevc,
                SupportsAudio = request.SupportsAudio,
                SupportsGamepad = request.SupportsGamepad,
                Capabilities = request.Capabilities ?? existingHost!.Capabilities,
                LastSeenUtc = now,
                UpdatedUtc = now,
                Availability = HostAvailability.Online,
            };
        }
        else
        {
            host = new HostRecord(
                HostId: $"host_{Guid.NewGuid():N}",
                HostSecret: CreateSecret(),
                DisplayName: request.DisplayName.Trim(),
                Region: Normalize(request.Region, "global"),
                DirectAddress: Normalize(request.DirectAddress, string.Empty),
                DirectPort: request.DirectPort,
                EncoderBackends: NormalizeList(request.EncoderBackends, Array.Empty<string>()),
                SupportsHevc: request.SupportsHevc,
                SupportsAudio: request.SupportsAudio,
                SupportsGamepad: request.SupportsGamepad,
                Capabilities: request.Capabilities ?? new HostCapabilitiesRequest(),
                Availability: HostAvailability.Online,
                ActiveSessionId: null,
                LastSeenUtc: now,
                CreatedUtc: now,
                UpdatedUtc: now);
        }

        state.Hosts[host.HostId] = host;

        return Results.Ok(new RegisterHostResponse(
            HostId: host.HostId,
            HostSecret: host.HostSecret,
            HeartbeatIntervalSeconds: 5,
            StreamEndpoint: BuildStreamEndpoint(host),
            Host: ToHostSummary(host, now)));
    }
}

static IResult PostHostHeartbeat(string hostId, HostHeartbeatRequest request, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Hosts.TryGetValue(hostId, out var host))
        {
            return Results.NotFound(new ApiError("host_not_found", $"Host '{hostId}' was not found."));
        }

        if (!FixedTimeEquals(host.HostSecret, request.HostSecret))
        {
            return Results.Unauthorized();
        }

        var repairedActiveSession = ResolveActiveHostSession(state, host, now);
        var retainedActiveSessionId = repairedActiveSession?.SessionId;
        if (string.IsNullOrWhiteSpace(retainedActiveSessionId) &&
            !string.IsNullOrWhiteSpace(host.ActiveSessionId) &&
            state.Sessions.TryGetValue(host.ActiveSessionId, out var existingActiveSession) &&
            existingActiveSession.Status is SessionStatus.Pending or SessionStatus.Active &&
            existingActiveSession.ExpiresUtc > now)
        {
            retainedActiveSessionId = existingActiveSession.SessionId;
        }

        var updated = host with
        {
            Availability = request.Availability ?? HostAvailability.Online,
            ActiveSessionId = retainedActiveSessionId,
            DirectAddress = Normalize(request.DirectAddress, host.DirectAddress),
            DirectPort = request.DirectPort > 0 ? request.DirectPort : host.DirectPort,
            LastSeenUtc = now,
            UpdatedUtc = now,
        };

        state.Hosts[hostId] = updated;
        state.Telemetry.Add(new TelemetryEventRecord(
            EventId: $"telemetry_{Guid.NewGuid():N}",
            EventType: "host_heartbeat",
            HostId: hostId,
            SessionId: updated.ActiveSessionId,
            Source: "host_agent",
            Payload: new Dictionary<string, object?>
            {
                ["cpuLoadPercent"] = request.CpuLoadPercent,
                ["gpuLoadPercent"] = request.GpuLoadPercent,
                ["networkKbps"] = request.NetworkKbps,
                ["availability"] = updated.Availability.ToString(),
            },
            RecordedUtc: now));

        return Results.Ok(new HostHeartbeatResponse(
            HostId: hostId,
            Availability: updated.Availability,
            Online: IsHostOnline(updated, now),
            ActiveSessionId: updated.ActiveSessionId,
            ServerUtc: now));
    }
}

static IResult GetHosts(HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!TryAuthorizeClientActor(httpRequest, state, out _, out var error))
        {
            return error!;
        }

        var hosts = state.Hosts.Values
            .OrderBy(host => host.DisplayName, StringComparer.OrdinalIgnoreCase)
            .Select(host => ToHostSummary(host, now))
            .ToArray();
        return Results.Ok(hosts);
    }
}

static IResult GetMarketplaceHosts(HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!TryAuthorizeClientActor(httpRequest, state, out _, out var error))
        {
            return error!;
        }

        var offers = state.HostOffers.Values
            .Where(offer => offer.Listed)
            .Select(offer => state.Hosts.TryGetValue(offer.HostId, out var host) ? ToMarketplaceHostOffer(host, offer, now) : null)
            .OfType<MarketplaceHostOfferResponse>()
            .OrderByDescending(offer => offer.Online)
            .ThenBy(offer => offer.Region, StringComparer.OrdinalIgnoreCase)
            .ThenBy(offer => offer.DisplayName, StringComparer.OrdinalIgnoreCase)
            .ToArray();
        return Results.Ok(offers);
    }
}

static IResult GetBillingSession(string sessionId, string? sessionToken, HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!TryAuthorizeSessionAction(httpRequest, state, session, sessionToken ?? string.Empty, out var error))
        {
            return error!;
        }

        state.BillingSessions.TryGetValue(sessionId, out var billing);
        return Results.Ok(ToBillingSessionDetails(session, billing, state, now));
    }
}

static IResult GetHostDetails(string hostId, HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!TryAuthorizeClientActor(httpRequest, state, out _, out var error))
        {
            return error!;
        }

        if (!state.Hosts.TryGetValue(hostId, out var host))
        {
            return Results.NotFound(new ApiError("host_not_found", $"Host '{hostId}' was not found."));
        }

        return Results.Ok(ToHostDetails(host, now));
    }
}

static IResult GetHostLease(string hostId, string hostSecret, ControlPlaneState state)
{
    if (string.IsNullOrWhiteSpace(hostSecret))
    {
        return Results.BadRequest(new ApiError("host_secret_required", "hostSecret is required."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);

        if (!state.Hosts.TryGetValue(hostId, out var host))
        {
            return Results.NotFound(new ApiError("host_not_found", $"Host '{hostId}' was not found."));
        }

        if (!FixedTimeEquals(host.HostSecret, hostSecret))
        {
            return Results.Unauthorized();
        }

        var session = ResolveActiveHostSession(state, host, now);
        if (session is null)
        {
            ControlPlaneDebugLog($"[lease] no lease for host={hostId}; activeSessionId={host.ActiveSessionId ?? "-"}");
            return Results.NoContent();
        }

        if (!string.Equals(host.ActiveSessionId, session.SessionId, StringComparison.Ordinal))
        {
            state.Hosts[hostId] = host with
            {
                ActiveSessionId = session.SessionId,
                Availability = HostAvailability.Busy,
                UpdatedUtc = now,
            };
        }

        ControlPlaneDebugLog($"[lease] active lease for host={hostId}; session={session.SessionId}; status={session.Status}; route={session.RouteKind}");
        return Results.Ok(ToHostLease(session, host));
    }
}

static IResult CreateSession(CreateSessionRequest request, HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options, IPaymentProvider paymentProvider)
{
    if (string.IsNullOrWhiteSpace(request.HostId))
    {
        return Results.BadRequest(new ApiError("host_id_required", "HostId is required."));
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!TryAuthorizeClientActor(httpRequest, state, out var actor, out var error))
        {
            return error!;
        }

        if (!TryResolveHostForSession(state, request.HostId, out var host, out var hostResolveError))
        {
            return hostResolveError ?? Results.NotFound(new ApiError("host_not_found", $"Host '{request.HostId}' was not found."));
        }

        if (TryFindActiveSessionForActor(state, actor, out var existingSession))
        {
            var stoppedExistingSession = existingSession! with
            {
                Status = SessionStatus.Stopped,
                StopReason = "actor_replaced",
                UpdatedUtc = now,
            };
            state.Sessions[stoppedExistingSession.SessionId] = stoppedExistingSession;

            if (state.Hosts.TryGetValue(stoppedExistingSession.HostId, out var existingHost))
            {
                CaptureBillingForSession(state, stoppedExistingSession, existingHost, options, paymentProvider, now, "actor_replaced");
                state.Hosts[stoppedExistingSession.HostId] = ReleaseHostAfterSessionStop(existingHost, now);
            }
        }

        foreach (var hostSession in state.Sessions.Values
                     .Where(session =>
                         string.Equals(session.HostId, host.HostId, StringComparison.Ordinal) &&
                         session.Status is SessionStatus.Pending or SessionStatus.Active)
                     .ToArray())
        {
            var supersededSession = hostSession with
            {
                Status = SessionStatus.Stopped,
                StopReason = "host_session_superseded",
                UpdatedUtc = now,
            };
            state.Sessions[hostSession.SessionId] = supersededSession;

            if (state.Hosts.TryGetValue(supersededSession.HostId, out var supersededHost))
            {
                CaptureBillingForSession(state, supersededSession, supersededHost, options, paymentProvider, now, "host_session_superseded");
                state.Hosts[supersededSession.HostId] = ReleaseHostAfterSessionStop(supersededHost, now);
            }
        }

        if (TryGetActorSessionCreateCooldownSeconds(state, actor, now, out var actorCreateCooldownSeconds))
        {
            return Results.Conflict(new ApiError("session_create_rate_limited", $"Session creation is rate-limited for this actor for {actorCreateCooldownSeconds}s."));
        }

        if (!IsHostOnline(host, now))
        {
            return Results.BadRequest(new ApiError("host_offline", $"Host '{request.HostId}' is offline."));
        }

        if (ResolveActiveHostSession(state, host, now) is not null ||
            state.Sessions.Values.Any(session =>
                string.Equals(session.HostId, host.HostId, StringComparison.Ordinal) &&
                session.Status is SessionStatus.Pending or SessionStatus.Active &&
                session.ExpiresUtc > now))
        {
            return Results.Conflict(new ApiError("host_busy", $"Host '{request.HostId}' already has an active session."));
        }

        var sessionId = $"session_{Guid.NewGuid():N}";
        var expiresUtc = now.AddMinutes(Math.Clamp(request.LeaseMinutes <= 0 ? 30 : request.LeaseMinutes, 5, 240));
        var route = SelectRoutePlan(state, host, request, now);
        var probeRelay = SelectProbeRelay(state, host, request, route, now);
        var negotiatedCodec = SelectSessionCodec(host, request);
        var session = new SessionRecord(
            SessionId: sessionId,
            SessionToken: CreateSecret(),
            HostId: host.HostId,
            ClientLabel: Normalize(request.ClientLabel, "anonymous"),
            ClientRegion: Normalize(request.ClientRegion, "global"),
            CodecPreference: negotiatedCodec,
            AudioRequested: request.AudioRequested,
            ControllerCount: Math.Clamp(request.ControllerCount <= 0 ? 1 : request.ControllerCount, 1, 4),
            LeaseMinutes: Math.Clamp(request.LeaseMinutes <= 0 ? 30 : request.LeaseMinutes, 5, 240),
            StreamEndpoint: BuildStreamEndpoint(host),
            ReceiverEndpoint: BuildReceiverEndpoint(request),
            DesiredStream: BuildDesiredStream(request),
            RouteKind: route.RouteKind,
            RouteState: ComputeRouteState(route.RouteKind, SessionStatus.Pending),
            RouteVersion: 1,
            RelayId: route.RelayId,
            RelayRegion: route.RelayRegion,
            RelayEndpoint: route.RelayEndpoint,
            CreatedByDeviceId: actor?.DeviceId,
            CreatedByDeviceLabel: actor?.DeviceLabel,
            CreatedByUserId: actor?.UserId,
            CreatedByUserEmail: actor?.UserEmail,
            UnattendedAuthorized: true,
            ProbeToken: probeRelay is null ? string.Empty : CreateSecret(),
            ProbeEndpoint: probeRelay is null ? null : BuildRelayEndpoint(probeRelay),
            NatStatus: probeRelay is null ? "probe_unavailable" : "probe_pending",
            HostNatProbe: null,
            ClientNatProbe: null,
            ReceiverRegisteredEndpoint: null,
            ReceiverRegisteredUtc: null,
            SenderRegisteredEndpoint: null,
            SenderRegisteredUtc: null,
            LastRouteActionKind: null,
            LastRouteActionReason: null,
            LastRouteActionActor: null,
            LastRouteActionUtc: null,
            RouteFallbackReadySinceUtc: null,
            RouteRecoveryReadySinceUtc: null,
            RouteRecoveryCount: 0,
            RouteRecoveryCooldownUntilUtc: null,
            RouteFallbackCount: 0,
            RouteFallbackCooldownUntilUtc: null,
            Status: SessionStatus.Pending,
            CreatedUtc: now,
            UpdatedUtc: now,
            ExpiresUtc: expiresUtc,
            StopReason: null);

        try
        {
            EnsureBillingHoldForSession(state, host, session, options, paymentProvider, now);
        }
        catch (Exception exception)
        {
            return Results.Problem(
                title: "Payment provider hold failed.",
                detail: NormalizePaymentProviderError(exception),
                statusCode: StatusCodes.Status502BadGateway,
                extensions: new Dictionary<string, object?> { ["code"] = "payment_provider_hold_failed" });
        }

        state.Sessions[sessionId] = session;
        state.Hosts[host.HostId] = host with
        {
            ActiveSessionId = sessionId,
            Availability = HostAvailability.Busy,
            UpdatedUtc = now,
        };
        ControlPlaneDebugLog(
            $"[create] session={sessionId}; host={host.HostId}; " +
            $"status={session.Status}; route={session.RouteKind}; receiver={session.ReceiverEndpoint?.Host ?? "-"}:{session.ReceiverEndpoint?.Port ?? 0}; " +
            $"codec={session.CodecPreference ?? "-"}; activeSessionId={sessionId}");

        return Results.Ok(ToSessionLease(session, host, state, now));
    }
}

static bool TryResolveHostForSession(ControlPlaneState state, string hostIdOrCode, out HostRecord host, out IResult? error)
{
    host = default!;
    error = null;

    var lookup = Normalize(hostIdOrCode, string.Empty);
    if (string.IsNullOrWhiteSpace(lookup))
    {
        error = Results.BadRequest(new ApiError("host_id_required", "HostId is required."));
        return false;
    }

    if (state.Hosts.TryGetValue(lookup, out host))
    {
        return true;
    }

    var code = lookup.Trim();
    if (code.Length is not 4)
    {
        return false;
    }

    var matches = state.Hosts.Values
        .Where(candidate => string.Equals(GetHostCode(candidate.HostId), code, StringComparison.OrdinalIgnoreCase))
        .ToArray();

    if (matches.Length == 1)
    {
        host = matches[0];
        return true;
    }

    if (matches.Length > 1)
    {
        error = Results.Conflict(new ApiError("host_code_ambiguous", $"Host code '{code}' matches multiple hosts. Use the full HostId."));
        return false;
    }

    return false;
}

static IResult GetSession(string sessionId, HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!TryAuthorizeClientActor(httpRequest, state, out _, out var error))
        {
            return error!;
        }

        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        state.Hosts.TryGetValue(session.HostId, out var host);
        return Results.Ok(ToSessionDetails(session, host, state, now));
    }
}

static IResult GetSessionConnectInstructions(string sessionId, string? sessionToken, HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!TryAuthorizeSessionAction(httpRequest, state, session, sessionToken ?? string.Empty, out var error))
        {
            return error!;
        }

        state.Hosts.TryGetValue(session.HostId, out var host);
        return Results.Ok(ToSessionConnectInstructions(session, host, state, now));
    }
}

static IResult PostSessionNatProbe(string sessionId, SessionNatProbeRequest request, HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!TryAuthorizeSessionAction(httpRequest, state, session, request.SessionToken, out var error))
        {
            return error!;
        }

        if (string.IsNullOrWhiteSpace(session.ProbeToken) ||
            !FixedTimeEquals(session.ProbeToken, request.ProbeToken ?? string.Empty))
        {
            return Results.BadRequest(new ApiError("probe_token_invalid", "ProbeToken is invalid for this session."));
        }

        if (string.IsNullOrWhiteSpace(request.ObservedAddress) || request.ObservedPort is < 1 or > 65535)
        {
            return Results.BadRequest(new ApiError("observed_endpoint_required", "ObservedAddress and a valid ObservedPort are required."));
        }

        var observation = new NatProbeObservation(
            ObservedAddress: request.ObservedAddress.Trim(),
            ObservedPort: request.ObservedPort,
            LocalAddress: NormalizeOptional(request.LocalAddress),
            LocalPort: request.LocalPort is > 0 and <= 65535 ? request.LocalPort : null,
            NetworkType: NormalizeOptional(request.NetworkType),
            ReportedUtc: now);

        var role = Normalize(request.Role, "client").ToLowerInvariant();
        var updated = role switch
        {
            "host" => session with { HostNatProbe = observation, UpdatedUtc = now },
            _ => session with { ClientNatProbe = observation, UpdatedUtc = now },
        };

        updated = UpdateNatRouteAndStatus(updated, role, observation, now);
        state.Sessions[sessionId] = updated;
        state.Hosts.TryGetValue(updated.HostId, out var host);
        return Results.Ok(ToSessionNatState(updated, host));
    }
}

static IResult PostSessionRelayRegistration(string sessionId, SessionRelayRegistrationRequest request, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (string.IsNullOrWhiteSpace(request.SessionToken) ||
            !FixedTimeEquals(session.SessionToken, request.SessionToken))
        {
            return Results.Unauthorized();
        }

        if (string.IsNullOrWhiteSpace(request.ObservedAddress) || request.ObservedPort is < 1 or > 65535)
        {
            return Results.BadRequest(new ApiError("observed_endpoint_required", "ObservedAddress and a valid ObservedPort are required."));
        }

        var observedEndpoint = new StreamEndpoint(
            request.ObservedAddress.Trim(),
            request.ObservedPort,
            "udp-evrt-observed");
        var role = Normalize(request.Role, string.Empty).ToLowerInvariant();

        var updated = role switch
        {
            "sender" => session with
            {
                SenderRegisteredEndpoint = observedEndpoint,
                SenderRegisteredUtc = now,
                UpdatedUtc = now,
            },
            _ => session with
            {
                ReceiverRegisteredEndpoint = observedEndpoint,
                ReceiverRegisteredUtc = now,
                UpdatedUtc = now,
            },
        };

        state.Sessions[sessionId] = updated;
        ControlPlaneDebugLog(
            $"[relay-register] session={sessionId}; role={role}; observed={observedEndpoint.Host}:{observedEndpoint.Port}; " +
            $"receiverRegistered={(updated.ReceiverRegisteredEndpoint is not null)}; senderRegistered={(updated.SenderRegisteredEndpoint is not null)}");
        state.Hosts.TryGetValue(updated.HostId, out var host);
        return Results.Ok(ToSessionConnectInstructions(updated, host, state, now));
    }
}

static IResult ActivateSession(string sessionId, SessionActionRequest request, HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!TryAuthorizeSessionAction(httpRequest, state, session, request.SessionToken, out var error))
        {
            return error!;
        }

        var updated = session with
        {
            Status = SessionStatus.Active,
            UpdatedUtc = now,
        };
        state.Sessions[sessionId] = updated;
        ControlPlaneDebugLog(
            $"[activate] session={sessionId}; host={updated.HostId}; status={updated.Status}; route={updated.RouteKind}; " +
            $"receiverRegistered={(updated.ReceiverRegisteredEndpoint is not null)}; hostReady={IsSessionReadyForHostStart(updated, now)}");
        state.Hosts.TryGetValue(updated.HostId, out var host);
        return Results.Ok(ToSessionDetails(updated, host, state, now));
    }
}

static IResult KeepAliveSession(string sessionId, SessionActionRequest request, HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!TryAuthorizeSessionAction(httpRequest, state, session, request.SessionToken, out var error))
        {
            return error!;
        }

        if (session.Status is SessionStatus.Stopped or SessionStatus.Expired)
        {
            return Results.Conflict(new ApiError("session_inactive", $"Session '{sessionId}' is no longer active."));
        }

        var updated = session with
        {
            ExpiresUtc = now.AddMinutes(Math.Clamp(session.LeaseMinutes <= 0 ? 30 : session.LeaseMinutes, 5, 240)),
            UpdatedUtc = now,
        };
        state.Sessions[sessionId] = updated;
        state.Hosts.TryGetValue(updated.HostId, out var host);
        return Results.Ok(ToSessionDetails(updated, host, state, now));
    }
}

static IResult FallbackSessionRoute(string sessionId, SessionActionRequest request, HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!TryAuthorizeSessionAction(httpRequest, state, session, request.SessionToken, out var error))
        {
            return error!;
        }

        if (session.Status is SessionStatus.Stopped or SessionStatus.Expired)
        {
            return Results.Conflict(new ApiError("session_inactive", $"Session '{sessionId}' is no longer active."));
        }

        if (!state.Hosts.TryGetValue(session.HostId, out var host))
        {
            return Results.NotFound(new ApiError("host_not_found", $"Host '{session.HostId}' was not found."));
        }

        if (TryGetRouteActionRateLimitSeconds(session, now, out var routeRateLimitSeconds))
        {
            return Results.Conflict(new ApiError("route_action_rate_limited", $"Route action is rate-limited for {routeRateLimitSeconds}s."));
        }

        var routeActionHint = ComputeRouteActionHint(session, host, state, now, out var routeActionReason);
        var fallbackCooldownSeconds = ComputeRouteFallbackCooldownSeconds(session, now);
        if (fallbackCooldownSeconds > 0)
        {
            return Results.Ok(ToSessionConnectInstructions(session, host, state, now));
        }

        var routeState = ComputeRouteState(session.RouteKind, session.Status);
        if (!string.Equals(routeActionHint, "fallback_recommended", StringComparison.OrdinalIgnoreCase) &&
            !string.Equals(routeState, "fallback", StringComparison.OrdinalIgnoreCase) &&
            !string.Equals(routeState, "degraded", StringComparison.OrdinalIgnoreCase))
        {
            return Results.Conflict(new ApiError("route_policy_blocked", $"Fallback is not currently recommended: {routeActionReason}."));
        }

        var route = SelectFallbackRoutePlan(state, host, session, now);
        var updated = ApplyFallbackRoutePlan(
            session,
            route,
            now,
            Normalize(request.Reason, "fallback_requested"),
            DescribeRouteActionActor(httpRequest, state, request.SessionToken));
        state.Sessions[sessionId] = updated;
        return Results.Ok(ToSessionConnectInstructions(updated, host, state, now));
    }
}

static IResult RecoverSessionRoute(string sessionId, SessionActionRequest request, HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!TryAuthorizeSessionAction(httpRequest, state, session, request.SessionToken, out var error))
        {
            return error!;
        }

        if (session.Status is SessionStatus.Stopped or SessionStatus.Expired)
        {
            return Results.Conflict(new ApiError("session_inactive", $"Session '{sessionId}' is no longer active."));
        }

        if (!state.Hosts.TryGetValue(session.HostId, out var host))
        {
            return Results.NotFound(new ApiError("host_not_found", $"Host '{session.HostId}' was not found."));
        }

        if (TryGetRouteActionRateLimitSeconds(session, now, out var routeRateLimitSeconds))
        {
            return Results.Conflict(new ApiError("route_action_rate_limited", $"Route action is rate-limited for {routeRateLimitSeconds}s."));
        }

        var routeActionHint = ComputeRouteActionHint(session, host, state, now, out var routeActionReason);
        var recoveryCooldownSeconds = ComputeRouteRecoveryCooldownSeconds(session, now);
        if (recoveryCooldownSeconds > 0)
        {
            return Results.Ok(ToSessionConnectInstructions(session, host, state, now));
        }

        if (!string.Equals(routeActionHint, "direct_recovery_recommended", StringComparison.OrdinalIgnoreCase))
        {
            return Results.Conflict(new ApiError("route_policy_blocked", $"Direct recovery is not currently recommended: {routeActionReason}."));
        }

        var route = SelectRecoveryRoutePlan(host, session);
        if (route is null)
        {
            return Results.Ok(ToSessionConnectInstructions(session, host, state, now));
        }

        var updated = ApplyRecoveryRoutePlan(
            session,
            route,
            now,
            Normalize(request.Reason, "direct_recovery_requested"),
            DescribeRouteActionActor(httpRequest, state, request.SessionToken));
        state.Sessions[sessionId] = updated;
        return Results.Ok(ToSessionConnectInstructions(updated, host, state, now));
    }
}

static IResult GetSessionRoutePolicy(string sessionId, string? sessionToken, HttpRequest httpRequest, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!TryAuthorizeSessionAction(httpRequest, state, session, sessionToken ?? string.Empty, out var error))
        {
            return error!;
        }

        state.Hosts.TryGetValue(session.HostId, out var host);
        return Results.Ok(ToSessionRoutePolicy(session, host, state, now));
    }
}

static IResult StopSession(string sessionId, SessionActionRequest request, HttpRequest httpRequest, ControlPlaneState state, ControlPlaneOptions options, IPaymentProvider paymentProvider)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);
        if (!state.Sessions.TryGetValue(sessionId, out var session))
        {
            return Results.NotFound(new ApiError("session_not_found", $"Session '{sessionId}' was not found."));
        }

        if (!TryAuthorizeSessionAction(httpRequest, state, session, request.SessionToken, out var error))
        {
            return error!;
        }

        var updatedSession = session with
        {
            Status = SessionStatus.Stopped,
            StopReason = Normalize(request.Reason, "manual_stop"),
            UpdatedUtc = now,
        };
        state.Sessions[sessionId] = updatedSession;
        ControlPlaneDebugLog($"[stop] session={sessionId}; host={session.HostId}; reason={updatedSession.StopReason}; priorStatus={session.Status}");

        if (state.Hosts.TryGetValue(session.HostId, out var hostForBilling))
        {
            CaptureBillingForSession(state, updatedSession, hostForBilling, options, paymentProvider, now, Normalize(request.Reason, "manual_stop"));
        }

        if (state.Hosts.TryGetValue(session.HostId, out var host))
        {
            state.Hosts[session.HostId] = ReleaseHostAfterSessionStop(host, now);
            ControlPlaneDebugLog($"[stop] host released; host={session.HostId}; activeSessionId=-");
        }

        var response = Results.Ok(ToSessionDetails(updatedSession, state.Hosts.GetValueOrDefault(updatedSession.HostId), state, now));
        state.Sessions.Remove(sessionId);
        ControlPlaneDebugLog($"[stop] session removed; session={sessionId}");
        return response;
    }
}

static IResult IngestTelemetry(TelemetryIngestRequest request, ControlPlaneState state)
{
    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        PruneExpiredState(state, now);

        if (!string.IsNullOrWhiteSpace(request.SessionId))
        {
            if (!state.Sessions.TryGetValue(request.SessionId, out var session))
            {
                return Results.Unauthorized();
            }

            if (session.Status is SessionStatus.Stopped or SessionStatus.Expired)
            {
                return Results.Conflict(new ApiError("session_inactive", $"Session '{request.SessionId}' is no longer active."));
            }

            if (string.IsNullOrWhiteSpace(request.SessionToken) ||
                !FixedTimeEquals(session.SessionToken, request.SessionToken))
            {
                return Results.Unauthorized();
            }

            if (!IsAllowedSessionTelemetry(request.EventType, request.Source))
            {
                return Results.BadRequest(new ApiError("telemetry_not_allowed", $"Telemetry event '{Normalize(request.EventType, "session_metric")}' from '{Normalize(request.Source, "unknown")}' is not allowed for sessions."));
            }

            if (!string.IsNullOrWhiteSpace(request.HostId) &&
                !string.Equals(session.HostId, request.HostId, StringComparison.OrdinalIgnoreCase))
            {
                return Results.Unauthorized();
            }
        }
    }

    var telemetryEvent = new TelemetryEventRecord(
        EventId: $"telemetry_{Guid.NewGuid():N}",
        EventType: Normalize(request.EventType, "session_metric"),
        HostId: request.HostId,
        SessionId: request.SessionId,
        Source: Normalize(request.Source, "unknown"),
        Payload: SanitizeTelemetryPayload(request.Payload),
        RecordedUtc: now);

    lock (state.SyncRoot)
    {
        if (!TryCoalesceTelemetryEvent(state, telemetryEvent))
        {
            state.Telemetry.Add(telemetryEvent);
        }

        if (!string.IsNullOrWhiteSpace(request.SessionId) &&
            state.Sessions.TryGetValue(request.SessionId, out var sessionForTelemetry))
        {
            state.Hosts.TryGetValue(sessionForTelemetry.HostId, out var hostForTelemetry);
            state.Sessions[sessionForTelemetry.SessionId] = UpdateRouteReadinessWindows(sessionForTelemetry, hostForTelemetry, state, now);
        }

        return Results.Accepted($"/api/telemetry/session/{telemetryEvent.EventId}", telemetryEvent);
    }
}

static void PruneExpiredState(ControlPlaneState state, DateTimeOffset now)
{
    var sessionActivityTimeout = TimeSpan.FromSeconds(15);

    foreach (var token in state.AccessTokens.Values.ToArray())
    {
        if (token.ExpiresUtc <= now)
        {
            state.AccessTokens.Remove(token.AccessToken);
        }
    }

    foreach (var token in state.RefreshTokens.Values.ToArray())
    {
        if (token.ExpiresUtc <= now)
        {
            state.RefreshTokens.Remove(token.RefreshToken);
        }
    }

    foreach (var token in state.UserAccessTokens.Values.ToArray())
    {
        if (token.ExpiresUtc <= now)
        {
            state.UserAccessTokens.Remove(token.AccessToken);
        }
    }

    foreach (var token in state.UserRefreshTokens.Values.ToArray())
    {
        if (token.ExpiresUtc <= now)
        {
            state.UserRefreshTokens.Remove(token.RefreshToken);
        }
    }

    foreach (var session in state.Sessions.Values.ToArray())
    {
        var leaseExpired = session.ExpiresUtc <= now;
        var activityExpired =
            session.Status is SessionStatus.Pending or SessionStatus.Active &&
            now - session.UpdatedUtc >= sessionActivityTimeout;

        if (session.Status is SessionStatus.Stopped or SessionStatus.Expired ||
            (!leaseExpired && !activityExpired))
        {
            continue;
        }

        var expired = session with
        {
            Status = SessionStatus.Expired,
            StopReason = activityExpired ? "session_idle_timeout" : "lease_expired",
            UpdatedUtc = now,
        };
        state.Sessions[session.SessionId] = expired;

        if (state.Hosts.TryGetValue(session.HostId, out var host) &&
            string.Equals(host.ActiveSessionId, session.SessionId, StringComparison.Ordinal))
        {
            state.Hosts[session.HostId] = ReleaseHostAfterSessionStop(host, now);
        }
    }

    var telemetryCutoff = now.AddDays(-14);
    if (state.Telemetry.Count > 0)
    {
        state.Telemetry.RemoveAll(eventRecord => eventRecord.RecordedUtc < telemetryCutoff);
        const int telemetryCap = 10_000;
        if (state.Telemetry.Count > telemetryCap)
        {
            var survivors = state.Telemetry
                .OrderByDescending(eventRecord => eventRecord.RecordedUtc)
                .Take(telemetryCap)
                .ToArray();
            state.Telemetry.Clear();
            state.Telemetry.AddRange(survivors.OrderBy(eventRecord => eventRecord.RecordedUtc));
        }
    }
}

static SessionRecord? ResolveActiveHostSession(ControlPlaneState state, HostRecord host, DateTimeOffset now)
{
    if (string.IsNullOrWhiteSpace(host.ActiveSessionId))
    {
        return null;
    }

    if (state.Sessions.TryGetValue(host.ActiveSessionId, out var activeSession) &&
        activeSession.Status is SessionStatus.Pending or SessionStatus.Active &&
        IsSessionReadyForHostStart(activeSession, now) &&
        activeSession.ExpiresUtc > now)
    {
        return activeSession;
    }

    return null;
}

static bool IsSessionReadyForHostStart(SessionRecord session, DateTimeOffset now)
{
    if (session.Status is not SessionStatus.Active)
    {
        return false;
    }

    if (!string.Equals(session.RouteKind, "relay_assigned", StringComparison.OrdinalIgnoreCase) &&
        !string.Equals(session.RouteKind, "relay_fallback", StringComparison.OrdinalIgnoreCase))
    {
        return true;
    }

    return session.ReceiverRegisteredEndpoint is not null &&
        session.ReceiverRegisteredUtc is not null &&
        now - session.ReceiverRegisteredUtc.Value <= TimeSpan.FromSeconds(10);
}

static bool TryAuthorizeClient(
    HttpRequest httpRequest,
    ControlPlaneState state,
    out DeviceAccessTokenRecord? accessToken,
    out DeviceRecord? device,
    out IResult? error)
{
    accessToken = null;
    device = null;
    error = null;

    if (!TryReadBearerToken(httpRequest, out var bearerToken))
    {
        error = Results.Unauthorized();
        return false;
    }

    if (!state.AccessTokens.TryGetValue(bearerToken, out var resolvedToken) || resolvedToken.ExpiresUtc <= DateTimeOffset.UtcNow)
    {
        error = Results.Unauthorized();
        return false;
    }

    if (!state.Devices.TryGetValue(resolvedToken.DeviceId, out var resolvedDevice))
    {
        error = Results.Unauthorized();
        return false;
    }

    accessToken = resolvedToken;
    device = resolvedDevice;
    return true;
}

static bool TryAuthorizeUser(
    HttpRequest httpRequest,
    ControlPlaneState state,
    out UserAccessTokenRecord? accessToken,
    out UserRecord? user,
    out IResult? error)
{
    accessToken = null;
    user = null;
    error = null;

    if (!TryReadBearerToken(httpRequest, out var bearerToken))
    {
        error = Results.Unauthorized();
        return false;
    }

    if (!state.UserAccessTokens.TryGetValue(bearerToken, out var resolvedToken) || resolvedToken.ExpiresUtc <= DateTimeOffset.UtcNow)
    {
        error = Results.Unauthorized();
        return false;
    }

    if (!state.Users.TryGetValue(resolvedToken.UserId, out var resolvedUser) || !resolvedUser.Enabled)
    {
        error = Results.Unauthorized();
        return false;
    }

    accessToken = resolvedToken;
    user = resolvedUser;
    return true;
}

static bool TryAuthorizeClientActor(
    HttpRequest httpRequest,
    ControlPlaneState state,
    out ClientActor? actor,
    out IResult? error)
{
    actor = null;
    error = null;

    if (TryAuthorizeUser(httpRequest, state, out var userToken, out var user, out _))
    {
        actor = new ClientActor(
            AuthKind: "user",
            DeviceId: null,
            DeviceLabel: null,
            UserId: user!.UserId,
            UserEmail: user.Email,
            ExpiresUtc: userToken!.ExpiresUtc);
        return true;
    }

    if (TryAuthorizeClient(httpRequest, state, out var deviceToken, out var device, out _))
    {
        actor = new ClientActor(
            AuthKind: "device",
            DeviceId: device!.DeviceId,
            DeviceLabel: device.DeviceLabel,
            UserId: null,
            UserEmail: null,
            ExpiresUtc: deviceToken!.ExpiresUtc);
        return true;
    }

    error = Results.Unauthorized();
    return false;
}

static bool TryAuthorizeSessionAction(
    HttpRequest httpRequest,
    ControlPlaneState state,
    SessionRecord session,
    string sessionToken,
    out IResult? error)
{
    if (TryAuthorizeClientActor(httpRequest, state, out var actor, out _))
    {
        if (!string.IsNullOrWhiteSpace(actor?.UserId) &&
            string.Equals(session.CreatedByUserId, actor.UserId, StringComparison.Ordinal))
        {
            error = null;
            return true;
        }

        if (!string.IsNullOrWhiteSpace(actor?.DeviceId) &&
            string.Equals(session.CreatedByDeviceId, actor.DeviceId, StringComparison.Ordinal))
        {
            error = null;
            return true;
        }
    }

    if (!string.IsNullOrWhiteSpace(sessionToken) && FixedTimeEquals(session.SessionToken, sessionToken))
    {
        error = null;
        return true;
    }

    error = Results.Unauthorized();
    return false;
}

static bool TryAuthorizeOperator(HttpRequest httpRequest, ControlPlaneOptions options, out IResult? error)
{
    error = null;

    if (!options.OperatorAuthConfigured)
    {
        error = Results.NotFound(new ApiError("operator_auth_disabled", "Operator API is disabled."));
        return false;
    }

    var operatorKey = httpRequest.Headers["X-Everty-Operator-Key"].ToString().Trim();
    if (string.IsNullOrWhiteSpace(operatorKey) &&
        TryReadBearerToken(httpRequest, out var bearerToken))
    {
        operatorKey = bearerToken;
    }

    if (string.IsNullOrWhiteSpace(operatorKey) ||
        !FixedTimeEquals(options.OperatorKey, operatorKey))
    {
        error = Results.Unauthorized();
        return false;
    }

    return true;
}

static bool TryReadBearerToken(HttpRequest httpRequest, out string bearerToken)
{
    bearerToken = string.Empty;
    var header = httpRequest.Headers.Authorization.ToString();
    const string prefix = "Bearer ";
    if (!header.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
    {
        return false;
    }

    bearerToken = header[prefix.Length..].Trim();
    return !string.IsNullOrWhiteSpace(bearerToken);
}

static bool IsHostOnline(HostRecord host, DateTimeOffset now) =>
    host.Availability != HostAvailability.Disabled &&
    now - host.LastSeenUtc <= TimeSpan.FromSeconds(60);

static bool IsRelayOnline(RelayRecord relay, DateTimeOffset now) =>
    relay.Availability != RelayAvailability.Disabled &&
    now - relay.LastSeenUtc <= TimeSpan.FromSeconds(15);

static HostRecord ReleaseHostAfterSessionStop(HostRecord host, DateTimeOffset now)
{
    var nextAvailability = host.Availability switch
    {
        HostAvailability.Disabled => HostAvailability.Disabled,
        HostAvailability.Offline => HostAvailability.Offline,
        _ => HostAvailability.Online,
    };

    return host with
    {
        ActiveSessionId = null,
        Availability = nextAvailability,
        LastSeenUtc = nextAvailability == HostAvailability.Online ? now : host.LastSeenUtc,
        UpdatedUtc = now,
    };
}

static string Normalize(string? value, string fallback) =>
    string.IsNullOrWhiteSpace(value) ? fallback : value.Trim();

static string GetBuildMarker() =>
    $"patched-{File.GetLastWriteTimeUtc(typeof(Program).Assembly.Location):yyyyMMddHHmmss}";

static string GetControlPlaneDebugLogPath()
{
    var directory = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "EvertyControlPlane");
    Directory.CreateDirectory(directory);
    return Path.Combine(directory, "control-plane-debug.log");
}

static void ControlPlaneDebugLog(string message)
{
    var line = $"[{DateTimeOffset.Now:yyyy-MM-dd HH:mm:ss.fff}] {message}";
    Console.WriteLine(line);
    try
    {
        File.AppendAllText(GetControlPlaneDebugLogPath(), line + Environment.NewLine);
    }
    catch
    {
    }
}

static string NormalizeEmail(string? value) =>
    string.IsNullOrWhiteSpace(value) ? string.Empty : value.Trim().ToLowerInvariant();

static string? NormalizeOptional(string? value) =>
    string.IsNullOrWhiteSpace(value) ? null : value.Trim();

static string NormalizeCurrency(string? value)
{
    var currency = Normalize(value, "USD").ToUpperInvariant();
    return currency.Length is >= 3 and <= 8 && currency.All(char.IsLetter)
        ? currency
        : "USD";
}

static string? NormalizeDescription(string? value)
{
    var description = NormalizeOptional(value);
    if (description is null)
    {
        return null;
    }

    return description.Length <= 240 ? description : description[..240];
}

static string DescribeSessionCreator(SessionRecord session)
{
    if (!string.IsNullOrWhiteSpace(session.CreatedByUserEmail))
    {
        return $"user:{session.CreatedByUserEmail}";
    }

    if (!string.IsNullOrWhiteSpace(session.CreatedByDeviceLabel))
    {
        return $"device:{session.CreatedByDeviceLabel}";
    }

    if (!string.IsNullOrWhiteSpace(session.CreatedByUserId))
    {
        return $"user:{session.CreatedByUserId}";
    }

    if (!string.IsNullOrWhiteSpace(session.CreatedByDeviceId))
    {
        return $"device:{session.CreatedByDeviceId}";
    }

    return "unknown";
}

static string[] NormalizeList(IEnumerable<string>? values, IEnumerable<string> fallback) =>
    values?.Where(static item => !string.IsNullOrWhiteSpace(item))
        .Select(static item => item.Trim())
        .Distinct(StringComparer.OrdinalIgnoreCase)
        .ToArray()
    ?? fallback.ToArray();

static string SelectSessionCodec(HostRecord host, CreateSessionRequest request)
{
    var preferred = NormalizeCodecNames(request.PreferredCodecs);
    if (preferred.Length == 0 && !string.IsNullOrWhiteSpace(request.CodecPreference))
    {
        preferred = NormalizeCodecNames(new[] { request.CodecPreference });
    }

    if (preferred.Length == 0)
    {
        preferred = new[] { "video/av1", "video/hevc", "video/avc" };
    }

    var hostSupported = GetHostSupportedEncodeCodecs(host);
    var clientSupported = GetClientSupportedDecodeCodecs(request);

    foreach (var codec in preferred)
    {
        if (hostSupported.Contains(codec, StringComparer.OrdinalIgnoreCase) &&
            clientSupported.Contains(codec, StringComparer.OrdinalIgnoreCase))
        {
            return codec;
        }
    }

    return hostSupported.Intersect(clientSupported, StringComparer.OrdinalIgnoreCase).FirstOrDefault()
           ?? "video/avc";
}

static string[] GetHostSupportedEncodeCodecs(HostRecord host)
{
    var configured = NormalizeCodecNames(host.Capabilities.SupportedEncodeCodecs);
    if (configured.Length > 0)
    {
        return configured;
    }

    var codecs = new List<string> { "video/avc" };
    if (host.SupportsHevc)
    {
        codecs.Add("video/hevc");
    }
    if (host.EncoderBackends.Any(static backend => backend.Contains("Av1", StringComparison.OrdinalIgnoreCase)))
    {
        codecs.Add("video/av1");
    }

    return codecs.Distinct(StringComparer.OrdinalIgnoreCase).ToArray();
}

static string[] GetClientSupportedDecodeCodecs(CreateSessionRequest request)
{
    var configured = NormalizeCodecNames(request.Capabilities?.SupportedDecodeCodecs);
    if (configured.Length > 0)
    {
        return configured;
    }

    return NormalizeCodecNames(request.PreferredCodecs).Length > 0
        ? NormalizeCodecNames(request.PreferredCodecs)
        : new[] { "video/avc", "video/hevc" };
}

static string[] NormalizeCodecNames(IEnumerable<string>? codecs) =>
    codecs?.Where(static codec => !string.IsNullOrWhiteSpace(codec))
        .Select(static codec => codec.Trim().ToLowerInvariant() switch
        {
            "av1" or "video/av1" => "video/av1",
            "hevc" or "h265" or "video/hevc" => "video/hevc",
            "avc" or "h264" or "video/avc" => "video/avc",
            var other => other,
        })
        .Distinct(StringComparer.OrdinalIgnoreCase)
        .ToArray()
    ?? Array.Empty<string>();

static string HashPassword(string salt, string password)
{
    using var sha = SHA256.Create();
    var bytes = System.Text.Encoding.UTF8.GetBytes($"{salt}:{password}");
    return Convert.ToHexString(sha.ComputeHash(bytes)).ToLowerInvariant();
}

static bool FixedTimeEquals(string left, string right)
{
    var leftBytes = System.Text.Encoding.UTF8.GetBytes(left);
    var rightBytes = System.Text.Encoding.UTF8.GetBytes(right);
    return leftBytes.Length == rightBytes.Length && CryptographicOperations.FixedTimeEquals(leftBytes, rightBytes);
}

static string CreateSecret()
{
    Span<byte> bytes = stackalloc byte[24];
    RandomNumberGenerator.Fill(bytes);
    return Convert.ToHexString(bytes).ToLowerInvariant();
}

static void RevokeDeviceTokens(ControlPlaneState state, string deviceId)
{
    foreach (var token in state.AccessTokens.Values.Where(token => string.Equals(token.DeviceId, deviceId, StringComparison.Ordinal)).ToArray())
    {
        state.AccessTokens.Remove(token.AccessToken);
    }

    foreach (var token in state.RefreshTokens.Values.Where(token => string.Equals(token.DeviceId, deviceId, StringComparison.Ordinal)).ToArray())
    {
        state.RefreshTokens.Remove(token.RefreshToken);
    }
}

static void RevokeUserTokens(ControlPlaneState state, string userId)
{
    foreach (var token in state.UserAccessTokens.Values.Where(token => string.Equals(token.UserId, userId, StringComparison.Ordinal)).ToArray())
    {
        state.UserAccessTokens.Remove(token.AccessToken);
    }

    foreach (var token in state.UserRefreshTokens.Values.Where(token => string.Equals(token.UserId, userId, StringComparison.Ordinal)).ToArray())
    {
        state.UserRefreshTokens.Remove(token.RefreshToken);
    }
}

static (DeviceAccessTokenRecord AccessToken, DeviceRefreshTokenRecord RefreshToken) IssueDeviceTokens(
    ControlPlaneState state,
    string deviceId,
    DateTimeOffset now,
    ControlPlaneOptions options)
{
    var accessToken = new DeviceAccessTokenRecord(
        AccessToken: CreateSecret(),
        DeviceId: deviceId,
        ExpiresUtc: now.Add(options.AccessTokenLifetime),
        CreatedUtc: now);
    state.AccessTokens[accessToken.AccessToken] = accessToken;

    var refreshToken = new DeviceRefreshTokenRecord(
        RefreshToken: CreateSecret(),
        DeviceId: deviceId,
        ExpiresUtc: now.Add(options.RefreshTokenLifetime),
        CreatedUtc: now);
    state.RefreshTokens[refreshToken.RefreshToken] = refreshToken;

    return (accessToken, refreshToken);
}

static (UserAccessTokenRecord AccessToken, UserRefreshTokenRecord RefreshToken) IssueUserTokens(
    ControlPlaneState state,
    string userId,
    DateTimeOffset now,
    ControlPlaneOptions options)
{
    var accessToken = new UserAccessTokenRecord(
        AccessToken: CreateSecret(),
        UserId: userId,
        ExpiresUtc: now.Add(options.AccessTokenLifetime),
        CreatedUtc: now);
    state.UserAccessTokens[accessToken.AccessToken] = accessToken;

    var refreshToken = new UserRefreshTokenRecord(
        RefreshToken: CreateSecret(),
        UserId: userId,
        ExpiresUtc: now.Add(options.RefreshTokenLifetime),
        CreatedUtc: now);
    state.UserRefreshTokens[refreshToken.RefreshToken] = refreshToken;

    return (accessToken, refreshToken);
}

static SessionRoutePlan SelectRoutePlan(ControlPlaneState state, HostRecord host, CreateSessionRequest request, DateTimeOffset now)
{
    var clientRegion = Normalize(request.ClientRegion, "global");
    var preferRelay = request.PreferRelay;
    var selectedRelay = SelectPreferredRelay(state, clientRegion, host.Region, now);

    if (ShouldPreferDirectLan(host, request))
    {
        return new SessionRoutePlan(
            RouteKind: "direct_host_push",
            RelayId: selectedRelay?.RelayId,
            RelayRegion: selectedRelay?.Region,
            RelayEndpoint: selectedRelay is null ? null : BuildRelayEndpoint(selectedRelay));
    }

    if (selectedRelay is not null)
    {
        return new SessionRoutePlan(
            RouteKind: "relay_assigned",
            RelayId: selectedRelay.RelayId,
            RelayRegion: selectedRelay.Region,
            RelayEndpoint: BuildRelayEndpoint(selectedRelay));
    }

    if (preferRelay)
    {
        return new SessionRoutePlan(
            RouteKind: "direct_fallback",
            RelayId: null,
            RelayRegion: null,
            RelayEndpoint: null);
    }

    return new SessionRoutePlan(
        RouteKind: "direct_host_push",
        RelayId: null,
        RelayRegion: null,
        RelayEndpoint: null);
}

static bool IsPrivateOrLocalAddress(string? address)
{
    if (string.IsNullOrWhiteSpace(address))
    {
        return true;
    }

    if (!IPAddress.TryParse(address.Trim(), out var ipAddress))
    {
        return false;
    }

    if (IPAddress.IsLoopback(ipAddress))
    {
        return true;
    }

    if (ipAddress.AddressFamily != AddressFamily.InterNetwork)
    {
        return false;
    }

    var bytes = ipAddress.GetAddressBytes();
    return bytes[0] == 10 ||
           (bytes[0] == 172 && bytes[1] is >= 16 and <= 31) ||
           (bytes[0] == 192 && bytes[1] == 168) ||
           (bytes[0] == 169 && bytes[1] == 254);
}

static bool ShouldPreferDirectLan(HostRecord host, CreateSessionRequest request)
{
    if (string.IsNullOrWhiteSpace(host.DirectAddress) || string.IsNullOrWhiteSpace(request.ReceiverAddress))
    {
        return false;
    }

    if (!IsPrivateOrLocalAddress(host.DirectAddress) || !IsPrivateOrLocalAddress(request.ReceiverAddress))
    {
        return false;
    }

    return AreLikelySameLan(host.DirectAddress, request.ReceiverAddress) ||
           request.Capabilities?.LanAddresses?.Any(candidate => AreLikelySameLan(host.DirectAddress, candidate)) == true;
}

static bool AreLikelySameLan(string leftAddress, string rightAddress)
{
    if (!IPAddress.TryParse(leftAddress.Trim(), out var left) || !IPAddress.TryParse(rightAddress.Trim(), out var right))
    {
        return false;
    }

    if (left.AddressFamily != AddressFamily.InterNetwork || right.AddressFamily != AddressFamily.InterNetwork)
    {
        return false;
    }

    var leftBytes = left.GetAddressBytes();
    var rightBytes = right.GetAddressBytes();

    if (leftBytes[0] == 10 && rightBytes[0] == 10)
    {
        return leftBytes[1] == rightBytes[1] && leftBytes[2] == rightBytes[2];
    }

    if (leftBytes[0] == 172 && rightBytes[0] == 172 &&
        leftBytes[1] is >= 16 and <= 31 &&
        rightBytes[1] is >= 16 and <= 31)
    {
        return leftBytes[1] == rightBytes[1] && leftBytes[2] == rightBytes[2];
    }

    if (leftBytes[0] == 192 && leftBytes[1] == 168 &&
        rightBytes[0] == 192 && rightBytes[1] == 168)
    {
        return leftBytes[2] == rightBytes[2];
    }

    return leftBytes[0] == rightBytes[0] &&
           leftBytes[1] == rightBytes[1] &&
           leftBytes[2] == rightBytes[2];
}

static SessionRoutePlan SelectFallbackRoutePlan(ControlPlaneState state, HostRecord host, SessionRecord session, DateTimeOffset now)
{
    var clientRegion = Normalize(session.ClientRegion, "global");
    var selectedRelay = SelectPreferredRelay(state, clientRegion, host.Region, now);

    if (selectedRelay is not null)
    {
        return new SessionRoutePlan(
            RouteKind: "relay_assigned",
            RelayId: selectedRelay.RelayId,
            RelayRegion: selectedRelay.Region,
            RelayEndpoint: BuildRelayEndpoint(selectedRelay));
    }

    return new SessionRoutePlan(
        RouteKind: "direct_fallback",
        RelayId: null,
        RelayRegion: null,
        RelayEndpoint: null);
}

static SessionRoutePlan? SelectRecoveryRoutePlan(HostRecord host, SessionRecord session)
{
    if (!string.Equals(session.NatStatus, "same_public_ip", StringComparison.OrdinalIgnoreCase))
    {
        return null;
    }

    if (session.HostNatProbe is null || session.ClientNatProbe is null)
    {
        return null;
    }

    if (!AreNatProbesFresh(session, DateTimeOffset.UtcNow))
    {
        return null;
    }

    if (session.RouteKind is "direct_punched" or "direct_host_push")
    {
        return null;
    }

    return new SessionRoutePlan(
        RouteKind: "direct_punched",
        RelayId: null,
        RelayRegion: null,
        RelayEndpoint: null);
}

static SessionRecord ApplyRoutePlan(SessionRecord session, SessionRoutePlan route, DateTimeOffset now) =>
    ApplyRoutePlanCore(session, route, now);

static SessionRecord ApplyRoutePlanCore(SessionRecord session, SessionRoutePlan route, DateTimeOffset now)
{
    var updated = session with
    {
        RouteKind = route.RouteKind,
        RouteState = ComputeRouteState(route.RouteKind, session.Status),
        RouteVersion = session.RouteVersion + 1,
        RelayId = route.RelayId,
        RelayRegion = route.RelayRegion,
        RelayEndpoint = route.RelayEndpoint,
        UpdatedUtc = now,
    };

    if (string.Equals(route.RouteKind, "direct_punched", StringComparison.OrdinalIgnoreCase) &&
        TryBuildDirectPunchedEndpoints(updated, out var punchedStreamEndpoint, out var punchedReceiverEndpoint))
    {
        updated = updated with
        {
            StreamEndpoint = punchedStreamEndpoint!,
            ReceiverEndpoint = punchedReceiverEndpoint!,
        };
    }

    return updated;
}

static SessionRecord ApplyFallbackRoutePlan(
    SessionRecord session,
    SessionRoutePlan route,
    DateTimeOffset now,
    string actionReason,
    string actionActor) =>
    ApplyRoutePlan(session, route, now) with
    {
        LastRouteActionKind = "fallback",
        LastRouteActionReason = actionReason,
        LastRouteActionActor = actionActor,
        LastRouteActionUtc = now,
        RouteFallbackCount = session.RouteFallbackCount + 1,
        RouteFallbackCooldownUntilUtc = now.AddSeconds(15),
        UpdatedUtc = now,
    };

static SessionRecord ApplyRecoveryRoutePlan(
    SessionRecord session,
    SessionRoutePlan route,
    DateTimeOffset now,
    string actionReason,
    string actionActor) =>
    ApplyRoutePlan(session, route, now) with
    {
        LastRouteActionKind = "recover",
        LastRouteActionReason = actionReason,
        LastRouteActionActor = actionActor,
        LastRouteActionUtc = now,
        RouteRecoveryCount = session.RouteRecoveryCount + 1,
        RouteRecoveryCooldownUntilUtc = now.AddSeconds(30),
        UpdatedUtc = now,
    };

static string ComputeRouteState(string routeKind, SessionStatus status) =>
    status switch
    {
        SessionStatus.Stopped or SessionStatus.Expired => "inactive",
        _ when string.Equals(routeKind, "relay_assigned", StringComparison.OrdinalIgnoreCase) => "fallback",
        _ when string.Equals(routeKind, "direct_fallback", StringComparison.OrdinalIgnoreCase) => "degraded",
        _ when string.Equals(routeKind, "direct_punched", StringComparison.OrdinalIgnoreCase) => "healthy",
        _ when string.Equals(routeKind, "direct_host_push", StringComparison.OrdinalIgnoreCase) => "healthy",
        _ => "syncing",
    };

static int ComputeRouteFallbackCooldownSeconds(SessionRecord session, DateTimeOffset now)
{
    if (session.RouteFallbackCooldownUntilUtc is null)
    {
        return 0;
    }

    var remaining = session.RouteFallbackCooldownUntilUtc.Value - now;
    return remaining <= TimeSpan.Zero ? 0 : (int)Math.Ceiling(remaining.TotalSeconds);
}

static int ComputeRouteRecoveryCooldownSeconds(SessionRecord session, DateTimeOffset now)
{
    if (session.RouteRecoveryCooldownUntilUtc is null)
    {
        return 0;
    }

    var remaining = session.RouteRecoveryCooldownUntilUtc.Value - now;
    return remaining <= TimeSpan.Zero ? 0 : (int)Math.Ceiling(remaining.TotalSeconds);
}

static int ComputeRouteFallbackReadyDurationSeconds(SessionRecord session, DateTimeOffset now)
{
    if (session.RouteFallbackReadySinceUtc is null)
    {
        return 0;
    }

    var elapsed = now - session.RouteFallbackReadySinceUtc.Value;
    return elapsed <= TimeSpan.Zero ? 0 : (int)Math.Floor(elapsed.TotalSeconds);
}

static int ComputeRouteRecoveryReadyDurationSeconds(SessionRecord session, DateTimeOffset now)
{
    if (session.RouteRecoveryReadySinceUtc is null)
    {
        return 0;
    }

    var elapsed = now - session.RouteRecoveryReadySinceUtc.Value;
    return elapsed <= TimeSpan.Zero ? 0 : (int)Math.Floor(elapsed.TotalSeconds);
}

static SessionRecord UpdateRouteReadinessWindows(SessionRecord session, HostRecord? host, ControlPlaneState state, DateTimeOffset now)
{
    var sessionHealth = ComputeSessionHealth(session, host, state, now, out _);
    var routeState = ComputeRouteState(session.RouteKind, session.Status);
    var transportLossLevel = ComputeTransportLossLevel(session, state, now);
    var transportAnomaly = ComputeTransportAnomaly(session, state, now);

    var fallbackReady = string.Equals(sessionHealth, "degraded", StringComparison.OrdinalIgnoreCase);
    var recoveryReady =
        (string.Equals(routeState, "fallback", StringComparison.OrdinalIgnoreCase) ||
         string.Equals(routeState, "degraded", StringComparison.OrdinalIgnoreCase)) &&
        string.Equals(transportLossLevel, "nominal", StringComparison.OrdinalIgnoreCase) &&
        IsNominalTransportAnomaly(transportAnomaly) &&
        string.Equals(session.NatStatus, "same_public_ip", StringComparison.OrdinalIgnoreCase) &&
        AreNatProbesFresh(session, now);

    return session with
    {
        RouteFallbackReadySinceUtc = fallbackReady
            ? session.RouteFallbackReadySinceUtc ?? now
            : null,
        RouteRecoveryReadySinceUtc = recoveryReady
            ? session.RouteRecoveryReadySinceUtc ?? now
            : null,
        UpdatedUtc = session.UpdatedUtc,
    };
}

static int ComputeNatProbeAgeSeconds(NatProbeObservation? observation, DateTimeOffset now)
{
    if (observation is null)
    {
        return -1;
    }

    var age = now - observation.ReportedUtc;
    return age <= TimeSpan.Zero ? 0 : (int)Math.Ceiling(age.TotalSeconds);
}

static bool AreNatProbesFresh(SessionRecord session, DateTimeOffset now)
{
    var hostAge = ComputeNatProbeAgeSeconds(session.HostNatProbe, now);
    var clientAge = ComputeNatProbeAgeSeconds(session.ClientNatProbe, now);
    return hostAge >= 0 && clientAge >= 0 && hostAge <= 60 && clientAge <= 60;
}

static bool TryGetRouteActionRateLimitSeconds(SessionRecord session, DateTimeOffset now, out int remainingSeconds)
{
    remainingSeconds = 0;
    if (session.LastRouteActionUtc is null)
    {
        return false;
    }

    var remaining = session.LastRouteActionUtc.Value.AddSeconds(5) - now;
    if (remaining <= TimeSpan.Zero)
    {
        return false;
    }

    remainingSeconds = (int)Math.Ceiling(remaining.TotalSeconds);
    return true;
}

static bool IsAllowedSessionTelemetry(string? eventType, string? source)
{
    var normalizedEventType = Normalize(eventType, "session_metric");
    var normalizedSource = Normalize(source, "unknown");
    return (normalizedEventType, normalizedSource) switch
    {
        ("receiver_feedback", "android_pc_receiver") => true,
        ("sender_snapshot", "receiver-native-host-agent") => true,
        _ => false,
    };
}

static Dictionary<string, object?> SanitizeTelemetryPayload(Dictionary<string, object?>? payload)
{
    const int maxKeys = 24;
    const int maxStringLength = 256;
    var result = new Dictionary<string, object?>(StringComparer.OrdinalIgnoreCase);
    if (payload is null)
    {
        return result;
    }

    foreach (var entry in payload)
    {
        if (result.Count >= maxKeys)
        {
            break;
        }

        var key = Normalize(entry.Key, string.Empty);
        if (string.IsNullOrWhiteSpace(key))
        {
            continue;
        }

        if (key.Length > 64)
        {
            key = key[..64];
        }

        result[key] = entry.Value switch
        {
            null => null,
            string s => s.Length <= maxStringLength ? s : s[..maxStringLength],
            bool b => b,
            byte or sbyte or short or ushort or int or uint or long or ulong or float or double or decimal => entry.Value,
            _ => entry.Value.ToString() is { } text
                ? (text.Length <= maxStringLength ? text : text[..maxStringLength])
                : null,
        };
    }

    return result;
}

static string DescribeRouteActionActor(HttpRequest httpRequest, ControlPlaneState state, string? sessionToken)
{
    if (TryAuthorizeClientActor(httpRequest, state, out var actor, out _))
    {
        if (!string.IsNullOrWhiteSpace(actor?.UserEmail))
        {
            return $"user:{actor.UserEmail}";
        }

        if (!string.IsNullOrWhiteSpace(actor?.DeviceLabel))
        {
            return $"device:{actor.DeviceLabel}";
        }

        if (!string.IsNullOrWhiteSpace(actor?.DeviceId))
        {
            return $"device:{actor.DeviceId}";
        }
    }

    if (!string.IsNullOrWhiteSpace(sessionToken))
    {
        return "session_token";
    }

    return "unknown";
}

static string ComputeRouteActionHint(SessionRecord session, HostRecord? host, ControlPlaneState state, DateTimeOffset now, out string reason)
{
    var fallbackCooldownSeconds = ComputeRouteFallbackCooldownSeconds(session, now);
    if (fallbackCooldownSeconds > 0)
    {
        reason = $"fallback cooldown {fallbackCooldownSeconds}s";
        return "cooldown_active";
    }

    var recoveryCooldownSeconds = ComputeRouteRecoveryCooldownSeconds(session, now);
    if (recoveryCooldownSeconds > 0 &&
        session.RouteKind is "relay_assigned" or "direct_fallback")
    {
        reason = $"recovery cooldown {recoveryCooldownSeconds}s";
        return "recovery_cooldown_active";
    }

    var routeState = ComputeRouteState(session.RouteKind, session.Status);
    var transportLossLevel = ComputeTransportLossLevel(session, state, now);
    var transportAnomaly = ComputeTransportAnomaly(session, state, now);
    if ((string.Equals(routeState, "fallback", StringComparison.OrdinalIgnoreCase) ||
         string.Equals(routeState, "degraded", StringComparison.OrdinalIgnoreCase)) &&
        string.Equals(session.NatStatus, "same_public_ip", StringComparison.OrdinalIgnoreCase) &&
        !AreNatProbesFresh(session, now))
    {
        reason = $"nat probes stale {ComputeNatProbeAgeSeconds(session.HostNatProbe, now)}s/{ComputeNatProbeAgeSeconds(session.ClientNatProbe, now)}s";
        return "wait_for_telemetry";
    }

    if ((string.Equals(routeState, "fallback", StringComparison.OrdinalIgnoreCase) ||
         string.Equals(routeState, "degraded", StringComparison.OrdinalIgnoreCase)) &&
        string.Equals(transportLossLevel, "nominal", StringComparison.OrdinalIgnoreCase) &&
        !IsNominalTransportAnomaly(transportAnomaly))
    {
        reason = $"direct recovery blocked by {transportAnomaly.Kind}: {transportAnomaly.Reason}";
        return "wait_for_telemetry";
    }

    if ((string.Equals(routeState, "fallback", StringComparison.OrdinalIgnoreCase) ||
         string.Equals(routeState, "degraded", StringComparison.OrdinalIgnoreCase)) &&
        string.Equals(transportLossLevel, "nominal", StringComparison.OrdinalIgnoreCase) &&
        IsNominalTransportAnomaly(transportAnomaly) &&
        string.Equals(session.NatStatus, "same_public_ip", StringComparison.OrdinalIgnoreCase) &&
        AreNatProbesFresh(session, now))
    {
        var recoveryReadyDurationSeconds = ComputeRouteRecoveryReadyDurationSeconds(session, now);
        if (recoveryReadyDurationSeconds >= 12)
        {
            reason = $"direct route recovery is available ({recoveryReadyDurationSeconds}s)";
            return "direct_recovery_recommended";
        }

        reason = $"direct recovery warming up {recoveryReadyDurationSeconds}s/12s";
        return "wait_for_telemetry";
    }

    var sessionHealth = ComputeSessionHealth(session, host, state, now, out var sessionHealthReason);
    if (string.Equals(sessionHealth, "syncing", StringComparison.OrdinalIgnoreCase))
    {
        reason = sessionHealthReason;
        return "wait_for_telemetry";
    }

    if (string.Equals(sessionHealth, "degraded", StringComparison.OrdinalIgnoreCase))
    {
        var fallbackReadyDurationSeconds = ComputeRouteFallbackReadyDurationSeconds(session, now);
        var fallbackWarmupSeconds = ComputeFallbackWarmupSeconds(transportAnomaly);
        if (fallbackReadyDurationSeconds >= fallbackWarmupSeconds)
        {
            reason = $"{sessionHealthReason} ({fallbackReadyDurationSeconds}s)";
            return "fallback_recommended";
        }

        reason = $"{sessionHealthReason} warming up {fallbackReadyDurationSeconds}s/{fallbackWarmupSeconds}s";
        return "wait_for_telemetry";
    }

    if (string.Equals(sessionHealth, "inactive", StringComparison.OrdinalIgnoreCase))
    {
        reason = sessionHealthReason;
        return "none";
    }

    reason = sessionHealthReason;
    return "none";
}

static int ComputeRecommendedSyncDelaySeconds(SessionRecord session, HostRecord? host, ControlPlaneState state, DateTimeOffset now)
{
    var fallbackCooldownSeconds = ComputeRouteFallbackCooldownSeconds(session, now);
    if (fallbackCooldownSeconds > 0)
    {
        return Math.Clamp(Math.Min(fallbackCooldownSeconds, 30), 5, 30);
    }

    var recoveryCooldownSeconds = ComputeRouteRecoveryCooldownSeconds(session, now);
    if (recoveryCooldownSeconds > 0)
    {
        return Math.Clamp(Math.Min(recoveryCooldownSeconds, 30), 5, 30);
    }

    var routeActionHint = ComputeRouteActionHint(session, host, state, now, out _);
    if (string.Equals(routeActionHint, "fallback_recommended", StringComparison.OrdinalIgnoreCase))
    {
        return 5;
    }

    if (string.Equals(routeActionHint, "direct_recovery_recommended", StringComparison.OrdinalIgnoreCase))
    {
        return 5;
    }

    if (string.Equals(routeActionHint, "wait_for_telemetry", StringComparison.OrdinalIgnoreCase))
    {
        return 15;
    }

    var transportAnomaly = ComputeTransportAnomaly(session, state, now);
    if (IsHighConfidenceTransportAnomaly(transportAnomaly))
    {
        return 5;
    }

    if (IsActionableTransportAnomaly(transportAnomaly))
    {
        return 8;
    }

    var sessionHealth = ComputeSessionHealth(session, host, state, now, out _);
    if (string.Equals(sessionHealth, "degraded", StringComparison.OrdinalIgnoreCase))
    {
        return 8;
    }

    if (string.Equals(sessionHealth, "syncing", StringComparison.OrdinalIgnoreCase))
    {
        return 15;
    }

    return 10;
}

static int ComputeTelemetryFreshnessSeconds(TelemetryEventRecord? telemetry, DateTimeOffset now)
{
    if (telemetry is null)
    {
        return -1;
    }

    var age = now - telemetry.RecordedUtc;
    return age <= TimeSpan.Zero ? 0 : (int)Math.Ceiling(age.TotalSeconds);
}

static string ComputeTransportLossLevel(SessionRecord session, ControlPlaneState state, DateTimeOffset now)
{
    var telemetry = GetLatestReceiverHealthTelemetry(state, session.SessionId);
    if (telemetry is null)
    {
        return "unknown";
    }

    var freshnessSeconds = ComputeTelemetryFreshnessSeconds(telemetry, now);
    if (freshnessSeconds > 45)
    {
        return "stale";
    }

    var payload = telemetry.Payload;
    if ((TryGetTelemetryString(payload, "receiverPressure", out var receiverPressure) ||
         TryGetTelemetryString(payload, "pressure", out receiverPressure)) &&
        string.Equals(receiverPressure, "critical", StringComparison.OrdinalIgnoreCase))
    {
        return "severe";
    }

    if (TryGetTelemetryInt(payload, "queueDropBurst", out var queueDropBurst) && queueDropBurst >= 3)
    {
        return "severe";
    }

    if ((TryGetTelemetryString(payload, "receiverPressure", out receiverPressure) ||
         TryGetTelemetryString(payload, "pressure", out receiverPressure)) &&
        string.Equals(receiverPressure, "high", StringComparison.OrdinalIgnoreCase))
    {
        return "elevated";
    }

    if (TryGetTelemetryInt(payload, "queueDropBurst", out queueDropBurst) && queueDropBurst >= 1)
    {
        return "elevated";
    }

    return "nominal";
}

static TransportAnomaly ComputeTransportAnomaly(SessionRecord session, ControlPlaneState state, DateTimeOffset now)
{
    var telemetry = GetLatestReceiverHealthTelemetry(state, session.SessionId);
    if (telemetry is null)
    {
        return new("awaiting_telemetry", "no receiver or sender telemetry yet", "low");
    }

    var freshnessSeconds = ComputeTelemetryFreshnessSeconds(telemetry, now);
    if (freshnessSeconds > 45)
    {
        return new("stale_telemetry", $"latest telemetry is {freshnessSeconds}s old", "high");
    }

    var payload = telemetry.Payload;
    if ((TryGetTelemetryString(payload, "receiverPressure", out var receiverPressure) ||
         TryGetTelemetryString(payload, "pressure", out receiverPressure)) &&
        string.Equals(receiverPressure, "critical", StringComparison.OrdinalIgnoreCase))
    {
        return new("receiver_pressure_critical", "receiver reported critical pressure", "high");
    }

    if (TryGetTelemetryInt(payload, "queueDropBurst", out var queueDropBurst) && queueDropBurst >= 3)
    {
        return new("queue_drop_burst", $"receiver dropped {queueDropBurst} queued frames in the last window", "high");
    }

    if (TryGetTelemetryInt(payload, "presentDeltaMs", out var presentDeltaMs) && presentDeltaMs >= 45)
    {
        return new("present_jitter", $"present delta {presentDeltaMs}ms", "high");
    }

    if (TryGetTelemetryInt(payload, "decodeDeltaMs", out var decodeDeltaMs) && decodeDeltaMs >= 45)
    {
        return new("decode_jitter", $"decode delta {decodeDeltaMs}ms", "medium");
    }

    if ((TryGetTelemetryString(payload, "receiverPressure", out receiverPressure) ||
         TryGetTelemetryString(payload, "pressure", out receiverPressure)) &&
        string.Equals(receiverPressure, "high", StringComparison.OrdinalIgnoreCase))
    {
        return new("receiver_pressure_high", "receiver reported high pressure", "medium");
    }

    if (TryGetTelemetryInt(payload, "queueDropBurst", out queueDropBurst) && queueDropBurst >= 1)
    {
        return new("queue_drop_burst", $"receiver dropped {queueDropBurst} queued frames in the last window", "medium");
    }

    var requestedFps = session.DesiredStream.RequestedFps ?? 0;
    if (requestedFps > 0 &&
        (TryGetTelemetryInt(payload, "receiverDecodeFps", out var receiverDecodeFps) ||
         TryGetTelemetryInt(payload, "decodeFps", out receiverDecodeFps)) &&
        receiverDecodeFps > 0 &&
        receiverDecodeFps < Math.Max(15, (int)Math.Floor(requestedFps * 0.82)))
    {
        return new("decode_fps_low", $"receiver decode {receiverDecodeFps} fps below requested {requestedFps} fps", "medium");
    }

    if (TryGetTelemetryInt(payload, "presentDeltaMs", out presentDeltaMs) && presentDeltaMs >= 28)
    {
        return new("present_jitter", $"present delta {presentDeltaMs}ms", "medium");
    }

    if (TryGetTelemetryInt(payload, "decodeDeltaMs", out decodeDeltaMs) && decodeDeltaMs >= 28)
    {
        return new("decode_jitter", $"decode delta {decodeDeltaMs}ms", "medium");
    }

    if (TryGetTelemetryInt(payload, "pulseEstimateMs", out var pulseEstimateMs) && pulseEstimateMs >= 70)
    {
        return new("video_tail_high", $"pulse estimate {pulseEstimateMs}ms", "medium");
    }

    if (TryGetTelemetryInt(payload, "inputEstimateMs", out var inputEstimateMs) && inputEstimateMs >= 90)
    {
        return new("input_tail_high", $"input estimate {inputEstimateMs}ms", "medium");
    }

    return new("nominal", "receiver telemetry nominal", "low");
}

static bool IsNominalTransportAnomaly(TransportAnomaly anomaly) =>
    string.Equals(anomaly.Kind, "nominal", StringComparison.OrdinalIgnoreCase);

static bool IsActionableTransportAnomaly(TransportAnomaly anomaly) =>
    !IsNominalTransportAnomaly(anomaly) &&
    !string.Equals(anomaly.Kind, "awaiting_telemetry", StringComparison.OrdinalIgnoreCase) &&
    !string.Equals(anomaly.Kind, "stale_telemetry", StringComparison.OrdinalIgnoreCase);

static bool IsHighConfidenceTransportAnomaly(TransportAnomaly anomaly) =>
    IsActionableTransportAnomaly(anomaly) &&
    string.Equals(anomaly.Confidence, "high", StringComparison.OrdinalIgnoreCase);

static int ComputeFallbackWarmupSeconds(TransportAnomaly anomaly) =>
    IsHighConfidenceTransportAnomaly(anomaly) ? 5 : 8;

static string ComputeSessionHealth(SessionRecord session, HostRecord? host, ControlPlaneState state, DateTimeOffset now, out string reason)
{
    if (session.Status is SessionStatus.Stopped or SessionStatus.Expired)
    {
        reason = "session inactive";
        return "inactive";
    }

    var routeState = ComputeRouteState(session.RouteKind, session.Status);
    if (string.Equals(routeState, "fallback", StringComparison.OrdinalIgnoreCase))
    {
        reason = "relay fallback route";
        return "degraded";
    }

    if (string.Equals(routeState, "degraded", StringComparison.OrdinalIgnoreCase))
    {
        reason = "direct fallback route";
        return "degraded";
    }

    if (host is null || !IsHostOnline(host, now))
    {
        if (string.Equals(session.RouteKind, "direct_host_push", StringComparison.OrdinalIgnoreCase) &&
            session.RelayEndpoint is not null)
        {
            reason = "host offline; relay available";
            return "fallback_recommended";
        }

        reason = "host offline";
        return "syncing";
    }

    var latestTelemetry = GetLatestReceiverHealthTelemetry(state, session.SessionId);
    if (latestTelemetry is null)
    {
        if (string.Equals(session.RouteKind, "direct_host_push", StringComparison.OrdinalIgnoreCase) &&
            session.RelayEndpoint is not null)
        {
            reason = "awaiting telemetry; relay available";
            return "fallback_recommended";
        }

        reason = "awaiting telemetry";
        return string.Equals(routeState, "healthy", StringComparison.OrdinalIgnoreCase) ? "syncing" : routeState;
    }

    var telemetryAge = now - latestTelemetry.RecordedUtc;
    if (telemetryAge > TimeSpan.FromSeconds(45))
    {
        reason = $"telemetry stale {telemetryAge.TotalSeconds:F0}s";
        return "syncing";
    }

    var transportAnomaly = ComputeTransportAnomaly(session, state, now);
    if (IsActionableTransportAnomaly(transportAnomaly))
    {
        reason = $"{transportAnomaly.Kind}: {transportAnomaly.Reason}";
        return "degraded";
    }

    var payload = latestTelemetry.Payload;
    if ((TryGetTelemetryString(payload, "receiverPressure", out var receiverPressure) ||
         TryGetTelemetryString(payload, "pressure", out receiverPressure)) &&
        (string.Equals(receiverPressure, "critical", StringComparison.OrdinalIgnoreCase) ||
         string.Equals(receiverPressure, "high", StringComparison.OrdinalIgnoreCase)))
    {
        reason = $"receiver pressure {receiverPressure}";
        return "degraded";
    }

    if (TryGetTelemetryInt(payload, "queueDropBurst", out var queueDropBurst) &&
        queueDropBurst >= 2)
    {
        reason = $"receiver queue drops {queueDropBurst}";
        return "degraded";
    }

    var requestedFps = session.DesiredStream.RequestedFps ?? 0;
    if (requestedFps > 0 &&
        (TryGetTelemetryInt(payload, "receiverDecodeFps", out var receiverDecodeFps) ||
         TryGetTelemetryInt(payload, "decodeFps", out receiverDecodeFps)) &&
        receiverDecodeFps > 0 &&
        receiverDecodeFps < Math.Max(15, (int)Math.Floor(requestedFps * 0.82)))
    {
        reason = $"receiver decode {receiverDecodeFps} fps";
        return "degraded";
    }

    if (requestedFps > 0 &&
        TryGetTelemetryInt(payload, "encodeFps", out var encodeFps) &&
        encodeFps > 0 &&
        encodeFps < Math.Max(15, (int)Math.Floor(requestedFps * 0.82)))
    {
        reason = $"encoder {encodeFps} fps";
        return "degraded";
    }

    if ((TryGetTelemetryInt(payload, "inputEstimateMs", out var inputEstimateMs) ||
         TryGetTelemetryInt(payload, "inputEstimateMs", out inputEstimateMs)) &&
        inputEstimateMs >= 85)
    {
        reason = $"input estimate {inputEstimateMs} ms";
        return "degraded";
    }

    if (TryGetTelemetryInt(payload, "pulseEstimateMs", out var pulseEstimateMs) &&
        pulseEstimateMs >= 70)
    {
        reason = $"pulse estimate {pulseEstimateMs} ms";
        return "degraded";
    }

    reason = "telemetry nominal";
    return "healthy";
}

static TelemetryEventRecord? GetLatestSessionTelemetry(ControlPlaneState state, string sessionId, string eventType) =>
    state.Telemetry
        .Where(eventRecord =>
            string.Equals(eventRecord.SessionId, sessionId, StringComparison.OrdinalIgnoreCase) &&
            string.Equals(eventRecord.EventType, eventType, StringComparison.OrdinalIgnoreCase))
        .OrderByDescending(eventRecord => eventRecord.RecordedUtc)
        .FirstOrDefault();

static TelemetryEventRecord? GetLatestReceiverHealthTelemetry(ControlPlaneState state, string sessionId)
{
    var receiverFeedback = GetLatestSessionTelemetry(state, sessionId, "receiver_feedback");
    if (receiverFeedback is not null)
    {
        return receiverFeedback;
    }

    return GetLatestSessionTelemetry(state, sessionId, "sender_snapshot");
}

static bool TryGetTelemetryString(Dictionary<string, object?> payload, string key, out string value)
{
    value = string.Empty;
    if (!payload.TryGetValue(key, out var raw) || raw is null)
    {
        return false;
    }

    switch (raw)
    {
        case string rawString:
            value = rawString;
            return !string.IsNullOrWhiteSpace(value);
        case JsonElement jsonElement when jsonElement.ValueKind == JsonValueKind.String:
            value = jsonElement.GetString() ?? string.Empty;
            return !string.IsNullOrWhiteSpace(value);
        case JsonElement jsonElement:
            value = jsonElement.ToString();
            return !string.IsNullOrWhiteSpace(value);
        default:
            value = raw.ToString() ?? string.Empty;
            return !string.IsNullOrWhiteSpace(value);
    }
}

static bool TryGetTelemetryInt(Dictionary<string, object?> payload, string key, out int value)
{
    value = 0;
    if (!payload.TryGetValue(key, out var raw) || raw is null)
    {
        return false;
    }

    switch (raw)
    {
        case int intValue:
            value = intValue;
            return true;
        case long longValue:
            value = (int)Math.Clamp(longValue, int.MinValue, int.MaxValue);
            return true;
        case double doubleValue:
            value = (int)Math.Round(doubleValue, MidpointRounding.AwayFromZero);
            return true;
        case float floatValue:
            value = (int)Math.Round(floatValue, MidpointRounding.AwayFromZero);
            return true;
        case decimal decimalValue:
            value = (int)Math.Round(decimalValue, MidpointRounding.AwayFromZero);
            return true;
        case JsonElement jsonElement when jsonElement.ValueKind == JsonValueKind.Number:
            if (jsonElement.TryGetInt32(out var jsonInt))
            {
                value = jsonInt;
                return true;
            }

            if (jsonElement.TryGetInt64(out var jsonLong))
            {
                value = (int)Math.Clamp(jsonLong, int.MinValue, int.MaxValue);
                return true;
            }

            if (jsonElement.TryGetDouble(out var jsonDouble))
            {
                value = (int)Math.Round(jsonDouble, MidpointRounding.AwayFromZero);
                return true;
            }

            break;
    }

    return int.TryParse(raw.ToString(), out value);
}

static RelayRecord? SelectProbeRelay(ControlPlaneState state, HostRecord host, CreateSessionRequest request, SessionRoutePlan route, DateTimeOffset now)
{
    if (!string.IsNullOrWhiteSpace(route.RelayId) &&
        state.Relays.TryGetValue(route.RelayId, out var assignedRelay) &&
        IsRelayOnline(assignedRelay, now))
    {
        return assignedRelay;
    }

    var clientRegion = Normalize(request.ClientRegion, "global");
    return SelectPreferredRelay(state, clientRegion, host.Region, now);
}

static RelayRecord? SelectPreferredRelay(ControlPlaneState state, string clientRegion, string hostRegion, DateTimeOffset now)
{
    var onlineRelays = state.Relays.Values
        .Where(relay => IsRelayOnline(relay, now))
        .ToArray();
    if (onlineRelays.Length == 0)
    {
        return null;
    }

    var unsaturated = onlineRelays
        .Where(relay => !IsRelaySaturated(state, relay.RelayId))
        .ToArray();
    var candidates = unsaturated.Length > 0 ? unsaturated : onlineRelays;

    return candidates
        .OrderByDescending(relay => string.Equals(relay.Region, clientRegion, StringComparison.OrdinalIgnoreCase))
        .ThenByDescending(relay => string.Equals(relay.Region, hostRegion, StringComparison.OrdinalIgnoreCase))
        .ThenBy(relay => GetRelayAssignedSessionCount(state, relay.RelayId))
        .ThenBy(relay => relay.DisplayName, StringComparer.OrdinalIgnoreCase)
        .FirstOrDefault();
}

static int GetRelayAssignedSessionCount(ControlPlaneState state, string relayId) =>
    state.Sessions.Values.Count(session =>
        session.Status is not (SessionStatus.Stopped or SessionStatus.Expired) &&
        string.Equals(session.RelayId, relayId, StringComparison.OrdinalIgnoreCase));

static bool IsRelaySaturated(ControlPlaneState state, string relayId) =>
    GetRelayAssignedSessionCount(state, relayId) >= 8;

static bool TryFindActiveSessionForActor(ControlPlaneState state, ClientActor? actor, out SessionRecord? session)
{
    session = null;
    if (actor is null)
    {
        return false;
    }

    session = state.Sessions.Values.FirstOrDefault(candidate =>
        candidate.Status is not (SessionStatus.Stopped or SessionStatus.Expired) &&
        ((!string.IsNullOrWhiteSpace(actor.UserId) &&
          string.Equals(candidate.CreatedByUserId, actor.UserId, StringComparison.Ordinal)) ||
         (!string.IsNullOrWhiteSpace(actor.DeviceId) &&
          string.Equals(candidate.CreatedByDeviceId, actor.DeviceId, StringComparison.Ordinal))));

    return session is not null;
}

static bool TryGetActorSessionCreateCooldownSeconds(ControlPlaneState state, ClientActor? actor, DateTimeOffset now, out int remainingSeconds)
{
    remainingSeconds = 0;
    if (actor is null)
    {
        return false;
    }

    var recentSession = state.Sessions.Values
        .Where(candidate =>
            ((!string.IsNullOrWhiteSpace(actor.UserId) &&
              string.Equals(candidate.CreatedByUserId, actor.UserId, StringComparison.Ordinal)) ||
             (!string.IsNullOrWhiteSpace(actor.DeviceId) &&
              string.Equals(candidate.CreatedByDeviceId, actor.DeviceId, StringComparison.Ordinal))))
        .OrderByDescending(candidate => candidate.CreatedUtc)
        .FirstOrDefault();

    if (recentSession is null)
    {
        return false;
    }

    var remaining = recentSession.CreatedUtc.AddSeconds(10) - now;
    if (remaining <= TimeSpan.Zero)
    {
        return false;
    }

    remainingSeconds = (int)Math.Ceiling(remaining.TotalSeconds);
    return true;
}

static bool TryCoalesceTelemetryEvent(ControlPlaneState state, TelemetryEventRecord telemetryEvent)
{
    if (string.IsNullOrWhiteSpace(telemetryEvent.SessionId))
    {
        return false;
    }

    if (!string.Equals(telemetryEvent.EventType, "receiver_feedback", StringComparison.OrdinalIgnoreCase) &&
        !string.Equals(telemetryEvent.EventType, "sender_snapshot", StringComparison.OrdinalIgnoreCase))
    {
        return false;
    }

    for (var index = state.Telemetry.Count - 1; index >= 0; index--)
    {
        var existing = state.Telemetry[index];
        if (!string.Equals(existing.SessionId, telemetryEvent.SessionId, StringComparison.OrdinalIgnoreCase) ||
            !string.Equals(existing.Source, telemetryEvent.Source, StringComparison.OrdinalIgnoreCase) ||
            !string.Equals(existing.EventType, telemetryEvent.EventType, StringComparison.OrdinalIgnoreCase))
        {
            continue;
        }

        if (telemetryEvent.RecordedUtc - existing.RecordedUtc > TimeSpan.FromSeconds(1))
        {
            return false;
        }

        state.Telemetry[index] = telemetryEvent;
        return true;
    }

    return false;
}

static StreamEndpoint BuildStreamEndpoint(HostRecord host) =>
    new(
        Host: string.IsNullOrWhiteSpace(host.DirectAddress) ? "manual" : host.DirectAddress,
        Port: host.DirectPort > 0 ? host.DirectPort : 5001,
        Transport: "udp-evrt-direct");

static StreamEndpoint BuildRelayEndpoint(RelayRecord relay) =>
    new(
        Host: relay.PublicAddress,
        Port: relay.UdpPort,
        Transport: "udp-evrt-relay");

static HostSummary ToHostSummary(HostRecord host, DateTimeOffset now) =>
    new(
        HostId: host.HostId,
        HostCode: GetHostCode(host.HostId),
        DisplayName: host.DisplayName,
        Region: host.Region,
        Online: IsHostOnline(host, now),
        Availability: host.Availability,
        ActiveSessionId: host.ActiveSessionId,
        StreamEndpoint: BuildStreamEndpoint(host),
        SupportsHevc: host.SupportsHevc,
        SupportsAudio: host.SupportsAudio,
        SupportsGamepad: host.SupportsGamepad,
        EncoderBackends: host.EncoderBackends,
        LastSeenUtc: host.LastSeenUtc);

static string GetHostCode(string hostId)
{
    const string prefix = "host_";
    if (string.IsNullOrWhiteSpace(hostId))
    {
        return string.Empty;
    }

    var trimmed = hostId.Trim();
    if (trimmed.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
    {
        var body = trimmed[prefix.Length..];
        return body.Length <= 4 ? body : body[..4];
    }

    return trimmed.Length <= 4 ? trimmed : trimmed[..4];
}

static MarketplaceHostOfferResponse ToMarketplaceHostOffer(HostRecord host, HostOfferRecord offer, DateTimeOffset now) =>
    new(
        HostId: host.HostId,
        DisplayName: host.DisplayName,
        Region: host.Region,
        Online: IsHostOnline(host, now),
        Availability: host.Availability,
        StreamEndpoint: BuildStreamEndpoint(host),
        SupportsHevc: host.SupportsHevc,
        SupportsAudio: host.SupportsAudio,
        SupportsGamepad: host.SupportsGamepad,
        EncoderBackends: host.EncoderBackends,
        PricePerHour: offer.PricePerHour,
        Currency: offer.Currency,
        Description: offer.Description,
        UpdatedUtc: offer.UpdatedUtc);

static BillingSessionDetails ToBillingSessionDetails(SessionRecord session, BillingSessionRecord? billing, ControlPlaneState state, DateTimeOffset now)
{
    var hold = billing?.HoldAmount ?? 0m;
    var captured = billing?.CapturedAmount ?? 0m;
    var settled = billing?.SettledAmount ?? 0m;
    var currency = billing?.Currency ?? GetHostBillingCurrency(state, session.HostId);
    return new BillingSessionDetails(
        SessionId: session.SessionId,
        HostId: session.HostId,
        Status: billing?.Status ?? BillingStatus.None,
        HoldAmount: hold,
        CapturedAmount: captured,
        SettledAmount: settled,
        Currency: currency,
        HourlyRate: billing?.HourlyRate ?? 0m,
        PlatformCommissionRate: billing?.PlatformCommissionRate ?? 0m,
        PaymentProvider: billing?.PaymentProvider ?? "manual",
        ProviderHoldId: billing?.ProviderHoldId,
        ProviderCaptureId: billing?.ProviderCaptureId,
        ProviderSettlementId: billing?.ProviderSettlementId,
        LastPaymentError: billing?.LastPaymentError,
        LastPaymentAttemptUtc: billing?.LastPaymentAttemptUtc,
        CreatedUtc: billing?.CreatedUtc ?? session.CreatedUtc,
        UpdatedUtc: billing?.UpdatedUtc ?? session.UpdatedUtc,
        SettledUtc: billing?.SettledUtc);
}

static BillingSummaryResponse BuildBillingSummary(ControlPlaneState state, DateTimeOffset now)
{
    var sessions = state.BillingSessions.Values.ToArray();
    return new BillingSummaryResponse(
        Service: "everty-control-plane",
        UtcNow: now,
        TotalHolds: sessions.Length,
        PendingHolds: sessions.Count(session => session.Status is BillingStatus.Held),
        CapturedHolds: sessions.Count(session => session.Status is BillingStatus.Captured or BillingStatus.Settled),
        SettledHolds: sessions.Count(session => session.Status is BillingStatus.Settled),
        HeldAmount: sessions.Sum(session => session.HoldAmount),
        CapturedAmount: sessions.Sum(session => session.CapturedAmount),
        SettledAmount: sessions.Sum(session => session.SettledAmount),
        LedgerEntries: state.BillingLedger.Count,
        Accounts: state.BillingAccounts.Count);
}

static BillingReconciliationItem ToBillingReconciliationItem(BillingSessionRecord billing, ControlPlaneState state, DateTimeOffset now)
{
    state.Sessions.TryGetValue(billing.SessionId, out var session);
    var action = session is null ? "inspect" : ResolveBillingReconciliationAction(session, billing);
    return new BillingReconciliationItem(
        SessionId: billing.SessionId,
        HostId: billing.HostId,
        BillingStatus: billing.Status,
        SessionStatus: session?.Status,
        PaymentProvider: billing.PaymentProvider,
        ActionRequired: action,
        HoldAmount: billing.HoldAmount,
        CapturedAmount: billing.CapturedAmount,
        SettledAmount: billing.SettledAmount,
        Currency: billing.Currency,
        ProviderHoldId: billing.ProviderHoldId,
        ProviderCaptureId: billing.ProviderCaptureId,
        ProviderSettlementId: billing.ProviderSettlementId,
        LastPaymentError: billing.LastPaymentError,
        LastPaymentAttemptUtc: billing.LastPaymentAttemptUtc,
        UpdatedUtc: billing.UpdatedUtc);
}

static string ResolveBillingReconciliationAction(SessionRecord session, BillingSessionRecord billing)
{
    if (billing.Status is BillingStatus.Failed)
    {
        return string.Equals(billing.Note, "settle_failed", StringComparison.OrdinalIgnoreCase) ||
            !string.IsNullOrWhiteSpace(billing.ProviderCaptureId)
            ? "settle"
            : "capture";
    }

    if (billing.Status is BillingStatus.Captured && string.IsNullOrWhiteSpace(billing.ProviderSettlementId))
    {
        return "settle";
    }

    if (billing.Status is BillingStatus.Held && session.Status is SessionStatus.Stopped or SessionStatus.Expired)
    {
        return "capture";
    }

    return "none";
}

static void EnsureBillingHoldForSession(ControlPlaneState state, HostRecord host, SessionRecord session, ControlPlaneOptions options, IPaymentProvider paymentProvider, DateTimeOffset now)
{
    var offer = state.HostOffers.TryGetValue(host.HostId, out var hostOffer) && hostOffer.Listed
        ? hostOffer
        : null;
    var currency = offer?.Currency ?? "USD";
    var hourlyRate = offer?.PricePerHour ?? 0m;
    var holdAmount = Math.Round(hourlyRate * Math.Max(1, session.LeaseMinutes) / 60m, 2, MidpointRounding.AwayFromZero);
    var platformCommissionRate = 0.15m;
    var providerHold = paymentProvider.ReserveHold(session.SessionId, host.HostId, holdAmount, currency, now);

    state.BillingAccounts[host.HostId] = state.BillingAccounts.TryGetValue(host.HostId, out var account)
        ? account with
        {
            Currency = currency,
            PendingAmount = Math.Round(account.PendingAmount + holdAmount, 2, MidpointRounding.AwayFromZero),
            UpdatedUtc = now,
        }
        : new BillingAccountRecord(
            HostId: host.HostId,
            Currency: currency,
            Balance: 0m,
            PendingAmount: holdAmount,
            PlatformCommissionRate: platformCommissionRate,
            CreatedUtc: now,
            UpdatedUtc: now);

    state.BillingSessions[session.SessionId] = new BillingSessionRecord(
        SessionId: session.SessionId,
        HostId: host.HostId,
        Status: BillingStatus.Held,
        HoldAmount: holdAmount,
        CapturedAmount: 0m,
        SettledAmount: 0m,
        Currency: currency,
        HourlyRate: hourlyRate,
        PlatformCommissionRate: platformCommissionRate,
        PaymentProvider: providerHold.Provider,
        ProviderHoldId: providerHold.ProviderReferenceId,
        ProviderCaptureId: null,
        ProviderSettlementId: null,
        LastPaymentError: null,
        LastPaymentAttemptUtc: now,
        CreatedUtc: now,
        UpdatedUtc: now,
        SettledUtc: null,
        Note: "hold_reserved");

    state.BillingLedger.Add(new BillingLedgerEntryRecord(
        EntryId: $"billing_{Guid.NewGuid():N}",
        SessionId: session.SessionId,
        HostId: host.HostId,
        Kind: "hold_reserved",
        Amount: holdAmount,
        Currency: currency,
        RecordedUtc: now,
        Note: $"lease_minutes={session.LeaseMinutes};provider={providerHold.Provider}"));
}

static void CaptureBillingForSession(ControlPlaneState state, SessionRecord session, HostRecord host, ControlPlaneOptions options, IPaymentProvider paymentProvider, DateTimeOffset now, string reason)
{
    if (!state.BillingSessions.TryGetValue(session.SessionId, out var billing))
    {
        EnsureBillingHoldForSession(state, host, session, options, paymentProvider, now);
        billing = state.BillingSessions[session.SessionId];
    }

    var offer = state.HostOffers.TryGetValue(host.HostId, out var hostOffer) && hostOffer.Listed
        ? hostOffer
        : null;
    var currency = billing.Currency;
    var hourlyRate = billing.HourlyRate > 0 ? billing.HourlyRate : offer?.PricePerHour ?? 0m;
    var elapsedMinutes = Math.Max(1, (int)Math.Ceiling(Math.Max(0, (now - session.CreatedUtc).TotalMinutes)));
    var capturedAmount = Math.Round(hourlyRate * elapsedMinutes / 60m, 2, MidpointRounding.AwayFromZero);
    var platformFee = Math.Round(capturedAmount * billing.PlatformCommissionRate, 2, MidpointRounding.AwayFromZero);
    var hostPayout = Math.Round(capturedAmount - platformFee, 2, MidpointRounding.AwayFromZero);
    PaymentProviderOperationResult providerCapture;
    try
    {
        providerCapture = paymentProvider.Capture(session.SessionId, host.HostId, capturedAmount, currency, billing.ProviderHoldId, now);
    }
    catch (Exception exception)
    {
        MarkBillingPaymentFailure(state, session, host, billing, "capture_failed", now, NormalizePaymentProviderError(exception));
        return;
    }

    state.BillingSessions[session.SessionId] = billing with
    {
        Status = BillingStatus.Captured,
        CapturedAmount = capturedAmount,
        SettledAmount = hostPayout,
        PaymentProvider = providerCapture.Provider,
        ProviderCaptureId = providerCapture.ProviderReferenceId,
        LastPaymentError = null,
        LastPaymentAttemptUtc = now,
        UpdatedUtc = now,
        Note = Normalize(reason, "billing_captured"),
    };

    state.BillingLedger.Add(new BillingLedgerEntryRecord(
        EntryId: $"billing_{Guid.NewGuid():N}",
        SessionId: session.SessionId,
        HostId: host.HostId,
        Kind: "capture",
        Amount: capturedAmount,
        Currency: currency,
        RecordedUtc: now,
        Note: $"host_payout={hostPayout:F2};platform_fee={platformFee:F2};provider={providerCapture.Provider};reason={reason}"));
}

static BillingSessionDetails SettleBillingSession(ControlPlaneState state, SessionRecord session, HostRecord host, BillingSessionRecord billing, ControlPlaneOptions options, IPaymentProvider paymentProvider, DateTimeOffset now, string reason)
{
    if (billing.Status is BillingStatus.Settled)
    {
        return ToBillingSessionDetails(session, billing, state, now);
    }

    var account = state.BillingAccounts.TryGetValue(host.HostId, out var existingAccount)
        ? existingAccount
        : new BillingAccountRecord(
            HostId: host.HostId,
            Currency: billing.Currency,
            Balance: 0m,
            PendingAmount: 0m,
            PlatformCommissionRate: billing.PlatformCommissionRate,
            CreatedUtc: now,
            UpdatedUtc: now);

    var updatedPending = Math.Round(Math.Max(0m, account.PendingAmount - billing.HoldAmount), 2, MidpointRounding.AwayFromZero);
    var updatedBalance = Math.Round(account.Balance + billing.SettledAmount, 2, MidpointRounding.AwayFromZero);
    PaymentProviderOperationResult providerSettlement;
    try
    {
        providerSettlement = paymentProvider.Settle(session.SessionId, host.HostId, billing.SettledAmount, billing.Currency, billing.ProviderCaptureId, now);
    }
    catch (Exception exception)
    {
        MarkBillingPaymentFailure(state, session, host, billing, "settle_failed", now, NormalizePaymentProviderError(exception));
        return ToBillingSessionDetails(session, state.BillingSessions[session.SessionId], state, now);
    }

    state.BillingAccounts[host.HostId] = account with
    {
        Currency = billing.Currency,
        PendingAmount = updatedPending,
        Balance = updatedBalance,
        UpdatedUtc = now,
    };

    state.BillingSessions[session.SessionId] = billing with
    {
        Status = BillingStatus.Settled,
        PaymentProvider = providerSettlement.Provider,
        ProviderSettlementId = providerSettlement.ProviderReferenceId,
        LastPaymentError = null,
        LastPaymentAttemptUtc = now,
        SettledUtc = now,
        UpdatedUtc = now,
        Note = Normalize(reason, "billing_settled"),
    };

    state.BillingLedger.Add(new BillingLedgerEntryRecord(
        EntryId: $"billing_{Guid.NewGuid():N}",
        SessionId: session.SessionId,
        HostId: host.HostId,
        Kind: "settle",
        Amount: billing.SettledAmount,
        Currency: billing.Currency,
        RecordedUtc: now,
        Note: $"balance={updatedBalance:F2};pending={updatedPending:F2};provider={providerSettlement.Provider};reason={reason}"));

    return ToBillingSessionDetails(session, state.BillingSessions[session.SessionId], state, now);
}

static void MarkBillingPaymentFailure(ControlPlaneState state, SessionRecord session, HostRecord host, BillingSessionRecord billing, string kind, DateTimeOffset now, string error)
{
    var normalizedError = Normalize(error, "payment_provider_failed");
    state.BillingSessions[session.SessionId] = billing with
    {
        Status = BillingStatus.Failed,
        LastPaymentError = normalizedError,
        LastPaymentAttemptUtc = now,
        UpdatedUtc = now,
        Note = kind,
    };

    state.BillingLedger.Add(new BillingLedgerEntryRecord(
        EntryId: $"billing_{Guid.NewGuid():N}",
        SessionId: session.SessionId,
        HostId: host.HostId,
        Kind: kind,
        Amount: kind is "capture_failed" ? billing.HoldAmount : billing.SettledAmount,
        Currency: billing.Currency,
        RecordedUtc: now,
        Note: $"provider={billing.PaymentProvider};error={normalizedError}"));
}

static string NormalizePaymentProviderError(Exception exception)
{
    var message = exception.GetBaseException().Message.Trim();
    return message.Length <= 240 ? message : message[..240];
}

static string GetHostBillingCurrency(ControlPlaneState state, string hostId)
{
    return state.HostOffers.TryGetValue(hostId, out var offer) && offer.Listed
        ? offer.Currency
        : "USD";
}

static RelaySummary ToRelaySummary(RelayRecord relay, ControlPlaneState state, DateTimeOffset now) =>
    new(
        RelayId: relay.RelayId,
        DisplayName: relay.DisplayName,
        Region: relay.Region,
        Online: IsRelayOnline(relay, now),
        Availability: relay.Availability,
        RelayEndpoint: BuildRelayEndpoint(relay),
        AssignedSessionCount: GetRelayAssignedSessionCount(state, relay.RelayId),
        Saturated: IsRelaySaturated(state, relay.RelayId),
        LastSeenUtc: relay.LastSeenUtc);

static UserSummary ToUserSummary(UserRecord user) =>
    new(
        UserId: user.UserId,
        Email: user.Email,
        CreatedUtc: user.CreatedUtc,
        LastSeenUtc: user.LastSeenUtc);

static DeviceSummary ToDeviceSummary(DeviceRecord device) =>
    new(
        DeviceId: device.DeviceId,
        DeviceLabel: device.DeviceLabel,
        Platform: device.Platform,
        CreatedUtc: device.CreatedUtc,
        LastSeenUtc: device.LastSeenUtc);

static HostDetails ToHostDetails(HostRecord host, DateTimeOffset now) =>
    new(
        Summary: ToHostSummary(host, now),
        Capabilities: host.Capabilities,
        UpdatedUtc: host.UpdatedUtc,
        CreatedUtc: host.CreatedUtc);

static SessionLeaseResponse ToSessionLease(SessionRecord session, HostRecord host, ControlPlaneState state, DateTimeOffset now)
{
    var sessionHealth = ComputeSessionHealth(session, host, state, now, out var sessionHealthReason);
    var routeActionHint = ComputeRouteActionHint(session, host, state, now, out var routeActionReason);
    var recommendedSyncDelaySeconds = ComputeRecommendedSyncDelaySeconds(session, host, state, now);
    var transportAnomaly = ComputeTransportAnomaly(session, state, now);
    var latestReceiverTelemetry = GetLatestSessionTelemetry(state, session.SessionId, "receiver_feedback");
    var latestSenderTelemetry = GetLatestSessionTelemetry(state, session.SessionId, "sender_snapshot");
    return new(
        SessionId: session.SessionId,
        SessionToken: session.SessionToken,
        HostId: session.HostId,
        Status: session.Status,
        StreamEndpoint: session.StreamEndpoint,
        ReceiverEndpoint: session.ReceiverEndpoint,
        RouteKind: session.RouteKind,
        RouteState: ComputeRouteState(session.RouteKind, session.Status),
        RouteVersion: session.RouteVersion,
        SessionHealth: sessionHealth,
        SessionHealthReason: sessionHealthReason,
        RouteActionHint: routeActionHint,
        RouteActionReason: routeActionReason,
        RouteFallbackReadyDurationSeconds: ComputeRouteFallbackReadyDurationSeconds(session, now),
        RouteRecoveryReadyDurationSeconds: ComputeRouteRecoveryReadyDurationSeconds(session, now),
        RecommendedSyncDelaySeconds: recommendedSyncDelaySeconds,
        TransportLossLevel: ComputeTransportLossLevel(session, state, now),
        TransportAnomalyKind: transportAnomaly.Kind,
        TransportAnomalyReason: transportAnomaly.Reason,
        TransportAnomalyConfidence: transportAnomaly.Confidence,
        ReceiverTelemetryAgeSeconds: ComputeTelemetryFreshnessSeconds(latestReceiverTelemetry, now),
        SenderTelemetryAgeSeconds: ComputeTelemetryFreshnessSeconds(latestSenderTelemetry, now),
        LastRouteActionKind: session.LastRouteActionKind,
        LastRouteActionReason: session.LastRouteActionReason,
        LastRouteActionActor: session.LastRouteActionActor,
        LastRouteActionUtc: session.LastRouteActionUtc,
        RouteRecoveryCount: session.RouteRecoveryCount,
        RouteRecoveryCooldownSeconds: ComputeRouteRecoveryCooldownSeconds(session, now),
        RouteFallbackCount: session.RouteFallbackCount,
        RouteFallbackCooldownSeconds: ComputeRouteFallbackCooldownSeconds(session, now),
        RelayEndpoint: session.RelayEndpoint,
        RelayRegion: session.RelayRegion,
        ProbeEndpoint: session.ProbeEndpoint,
        ProbeToken: session.ProbeToken,
        NatStatus: session.NatStatus,
        HostNatProbeAgeSeconds: ComputeNatProbeAgeSeconds(session.HostNatProbe, now),
        ClientNatProbeAgeSeconds: ComputeNatProbeAgeSeconds(session.ClientNatProbe, now),
        NatProbeFresh: AreNatProbesFresh(session, now),
        HostNatProbe: session.HostNatProbe,
        ClientNatProbe: session.ClientNatProbe,
        DesiredStream: session.DesiredStream,
        CodecPreference: session.CodecPreference,
        AudioRequested: session.AudioRequested,
        ControllerCount: session.ControllerCount,
        HostDisplayName: host.DisplayName,
        ExpiresUtc: session.ExpiresUtc);
}

static SessionDetails ToSessionDetails(SessionRecord session, HostRecord? host, ControlPlaneState state, DateTimeOffset now)
{
    var sessionHealth = ComputeSessionHealth(session, host, state, now, out var sessionHealthReason);
    var routeActionHint = ComputeRouteActionHint(session, host, state, now, out var routeActionReason);
    var recommendedSyncDelaySeconds = ComputeRecommendedSyncDelaySeconds(session, host, state, now);
    var transportAnomaly = ComputeTransportAnomaly(session, state, now);
    var latestReceiverTelemetry = GetLatestSessionTelemetry(state, session.SessionId, "receiver_feedback");
    var latestSenderTelemetry = GetLatestSessionTelemetry(state, session.SessionId, "sender_snapshot");
    return new(
        SessionId: session.SessionId,
        HostId: session.HostId,
        HostDisplayName: host?.DisplayName ?? session.HostId,
        Status: session.Status,
        StreamEndpoint: session.StreamEndpoint,
        ReceiverEndpoint: session.ReceiverEndpoint,
        RouteKind: session.RouteKind,
        RouteState: ComputeRouteState(session.RouteKind, session.Status),
        RouteVersion: session.RouteVersion,
        SessionHealth: sessionHealth,
        SessionHealthReason: sessionHealthReason,
        RouteActionHint: routeActionHint,
        RouteActionReason: routeActionReason,
        RouteFallbackReadyDurationSeconds: ComputeRouteFallbackReadyDurationSeconds(session, now),
        RouteRecoveryReadyDurationSeconds: ComputeRouteRecoveryReadyDurationSeconds(session, now),
        RecommendedSyncDelaySeconds: recommendedSyncDelaySeconds,
        TransportLossLevel: ComputeTransportLossLevel(session, state, now),
        TransportAnomalyKind: transportAnomaly.Kind,
        TransportAnomalyReason: transportAnomaly.Reason,
        TransportAnomalyConfidence: transportAnomaly.Confidence,
        ReceiverTelemetryAgeSeconds: ComputeTelemetryFreshnessSeconds(latestReceiverTelemetry, now),
        SenderTelemetryAgeSeconds: ComputeTelemetryFreshnessSeconds(latestSenderTelemetry, now),
        LastRouteActionKind: session.LastRouteActionKind,
        LastRouteActionReason: session.LastRouteActionReason,
        LastRouteActionActor: session.LastRouteActionActor,
        LastRouteActionUtc: session.LastRouteActionUtc,
        RouteRecoveryCount: session.RouteRecoveryCount,
        RouteRecoveryCooldownSeconds: ComputeRouteRecoveryCooldownSeconds(session, now),
        RouteFallbackCount: session.RouteFallbackCount,
        RouteFallbackCooldownSeconds: ComputeRouteFallbackCooldownSeconds(session, now),
        RelayEndpoint: session.RelayEndpoint,
        RelayRegion: session.RelayRegion,
        ProbeEndpoint: session.ProbeEndpoint,
        ProbeToken: session.ProbeToken,
        NatStatus: session.NatStatus,
        HostNatProbeAgeSeconds: ComputeNatProbeAgeSeconds(session.HostNatProbe, now),
        ClientNatProbeAgeSeconds: ComputeNatProbeAgeSeconds(session.ClientNatProbe, now),
        NatProbeFresh: AreNatProbesFresh(session, now),
        HostNatProbe: session.HostNatProbe,
        ClientNatProbe: session.ClientNatProbe,
        DesiredStream: session.DesiredStream,
        CodecPreference: session.CodecPreference,
        AudioRequested: session.AudioRequested,
        ControllerCount: session.ControllerCount,
        CreatedUtc: session.CreatedUtc,
        UpdatedUtc: session.UpdatedUtc,
        ExpiresUtc: session.ExpiresUtc,
        StopReason: session.StopReason);
}

static SessionConnectInstructions ToSessionConnectInstructions(SessionRecord session, HostRecord? host, ControlPlaneState state, DateTimeOffset now)
{
    var sessionHealth = ComputeSessionHealth(session, host, state, now, out var sessionHealthReason);
    var routeActionHint = ComputeRouteActionHint(session, host, state, now, out var routeActionReason);
    var recommendedSyncDelaySeconds = ComputeRecommendedSyncDelaySeconds(session, host, state, now);
    var transportAnomaly = ComputeTransportAnomaly(session, state, now);
    var latestReceiverTelemetry = GetLatestSessionTelemetry(state, session.SessionId, "receiver_feedback");
    var latestSenderTelemetry = GetLatestSessionTelemetry(state, session.SessionId, "sender_snapshot");
    return new(
        SessionId: session.SessionId,
        HostId: session.HostId,
        HostDisplayName: host?.DisplayName ?? session.HostId,
        Status: session.Status,
        RouteKind: session.RouteKind,
        RouteState: ComputeRouteState(session.RouteKind, session.Status),
        RouteVersion: session.RouteVersion,
        SessionHealth: sessionHealth,
        SessionHealthReason: sessionHealthReason,
        RouteActionHint: routeActionHint,
        RouteActionReason: routeActionReason,
        RouteFallbackReadyDurationSeconds: ComputeRouteFallbackReadyDurationSeconds(session, now),
        RouteRecoveryReadyDurationSeconds: ComputeRouteRecoveryReadyDurationSeconds(session, now),
        RecommendedSyncDelaySeconds: recommendedSyncDelaySeconds,
        TransportLossLevel: ComputeTransportLossLevel(session, state, now),
        TransportAnomalyKind: transportAnomaly.Kind,
        TransportAnomalyReason: transportAnomaly.Reason,
        TransportAnomalyConfidence: transportAnomaly.Confidence,
        ReceiverTelemetryAgeSeconds: ComputeTelemetryFreshnessSeconds(latestReceiverTelemetry, now),
        SenderTelemetryAgeSeconds: ComputeTelemetryFreshnessSeconds(latestSenderTelemetry, now),
        LastRouteActionKind: session.LastRouteActionKind,
        LastRouteActionReason: session.LastRouteActionReason,
        LastRouteActionActor: session.LastRouteActionActor,
        LastRouteActionUtc: session.LastRouteActionUtc,
        RouteRecoveryCount: session.RouteRecoveryCount,
        RouteRecoveryCooldownSeconds: ComputeRouteRecoveryCooldownSeconds(session, now),
        RouteFallbackCount: session.RouteFallbackCount,
        RouteFallbackCooldownSeconds: ComputeRouteFallbackCooldownSeconds(session, now),
        StreamEndpoint: session.StreamEndpoint,
        ReceiverEndpoint: session.ReceiverEndpoint,
        RelayEndpoint: session.RelayEndpoint,
        RelayRegion: session.RelayRegion,
        ProbeEndpoint: session.ProbeEndpoint,
        ProbeToken: session.ProbeToken,
        NatStatus: session.NatStatus,
        ReceiverRegistered: session.ReceiverRegisteredEndpoint is not null && session.ReceiverRegisteredUtc is not null && now - session.ReceiverRegisteredUtc.Value <= TimeSpan.FromSeconds(10),
        HostReady: IsSessionReadyForHostStart(session, now),
        ReceiverRegisteredEndpoint: session.ReceiverRegisteredEndpoint,
        SenderRegisteredEndpoint: session.SenderRegisteredEndpoint,
        HostNatProbeAgeSeconds: ComputeNatProbeAgeSeconds(session.HostNatProbe, now),
        ClientNatProbeAgeSeconds: ComputeNatProbeAgeSeconds(session.ClientNatProbe, now),
        NatProbeFresh: AreNatProbesFresh(session, now),
        HostNatProbe: session.HostNatProbe,
        ClientNatProbe: session.ClientNatProbe,
        ExpiresUtc: session.ExpiresUtc);
}

static SessionRoutePolicyResponse ToSessionRoutePolicy(SessionRecord session, HostRecord? host, ControlPlaneState state, DateTimeOffset now)
{
    var sessionHealth = ComputeSessionHealth(session, host, state, now, out var sessionHealthReason);
    var routeActionHint = ComputeRouteActionHint(session, host, state, now, out var routeActionReason);
    var transportAnomaly = ComputeTransportAnomaly(session, state, now);
    var latestReceiverTelemetry = GetLatestSessionTelemetry(state, session.SessionId, "receiver_feedback");
    var latestSenderTelemetry = GetLatestSessionTelemetry(state, session.SessionId, "sender_snapshot");
    var fallbackWarmupSeconds = ComputeFallbackWarmupSeconds(transportAnomaly);
    var recoveryWarmupSeconds = 12;
    var fallbackReadyDurationSeconds = ComputeRouteFallbackReadyDurationSeconds(session, now);
    var recoveryReadyDurationSeconds = ComputeRouteRecoveryReadyDurationSeconds(session, now);

    return new(
        SessionId: session.SessionId,
        HostId: session.HostId,
        RouteKind: session.RouteKind,
        RouteState: ComputeRouteState(session.RouteKind, session.Status),
        RouteVersion: session.RouteVersion,
        SessionHealth: sessionHealth,
        SessionHealthReason: sessionHealthReason,
        RouteActionHint: routeActionHint,
        RouteActionReason: routeActionReason,
        RecommendedSyncDelaySeconds: ComputeRecommendedSyncDelaySeconds(session, host, state, now),
        TransportLossLevel: ComputeTransportLossLevel(session, state, now),
        TransportAnomalyKind: transportAnomaly.Kind,
        TransportAnomalyReason: transportAnomaly.Reason,
        TransportAnomalyConfidence: transportAnomaly.Confidence,
        ActionableAnomaly: IsActionableTransportAnomaly(transportAnomaly),
        HighConfidenceAnomaly: IsHighConfidenceTransportAnomaly(transportAnomaly),
        FallbackWarmupSeconds: fallbackWarmupSeconds,
        FallbackReadyDurationSeconds: fallbackReadyDurationSeconds,
        FallbackReady: fallbackReadyDurationSeconds >= fallbackWarmupSeconds,
        RecoveryWarmupSeconds: recoveryWarmupSeconds,
        RecoveryReadyDurationSeconds: recoveryReadyDurationSeconds,
        RecoveryReady: recoveryReadyDurationSeconds >= recoveryWarmupSeconds,
        FallbackCooldownSeconds: ComputeRouteFallbackCooldownSeconds(session, now),
        RecoveryCooldownSeconds: ComputeRouteRecoveryCooldownSeconds(session, now),
        ReceiverTelemetryAgeSeconds: ComputeTelemetryFreshnessSeconds(latestReceiverTelemetry, now),
        SenderTelemetryAgeSeconds: ComputeTelemetryFreshnessSeconds(latestSenderTelemetry, now),
        NatStatus: session.NatStatus,
        HostNatProbeAgeSeconds: ComputeNatProbeAgeSeconds(session.HostNatProbe, now),
        ClientNatProbeAgeSeconds: ComputeNatProbeAgeSeconds(session.ClientNatProbe, now),
        NatProbeFresh: AreNatProbesFresh(session, now));
}

static HostLeaseResponse ToHostLease(SessionRecord session, HostRecord host) =>
    new(
        HostId: host.HostId,
        SessionId: session.SessionId,
        SessionToken: session.SessionToken,
        ClientLabel: session.ClientLabel,
        Status: session.Status,
        StreamEndpoint: session.StreamEndpoint,
        ReceiverEndpoint: session.ReceiverEndpoint,
        RouteKind: session.RouteKind,
        RelayEndpoint: session.RelayEndpoint,
        RouteVersion: session.RouteVersion,
        RelayRegion: session.RelayRegion,
        ProbeEndpoint: session.ProbeEndpoint,
        ProbeToken: session.ProbeToken,
        NatStatus: session.NatStatus,
        ReceiverRegistered: session.ReceiverRegisteredEndpoint is not null &&
            session.ReceiverRegisteredUtc is not null &&
            DateTimeOffset.UtcNow - session.ReceiverRegisteredUtc.Value <= TimeSpan.FromSeconds(10),
        HostReady: IsSessionReadyForHostStart(session, DateTimeOffset.UtcNow),
        HostNatProbe: session.HostNatProbe,
        ClientNatProbe: session.ClientNatProbe,
        DesiredStream: session.DesiredStream,
        UnattendedAuthorized: session.UnattendedAuthorized,
        CodecPreference: session.CodecPreference,
        AudioRequested: session.AudioRequested,
        ControllerCount: session.ControllerCount,
        CreatedUtc: session.CreatedUtc,
        UpdatedUtc: session.UpdatedUtc,
        ExpiresUtc: session.ExpiresUtc);

static SessionNatStateResponse ToSessionNatState(SessionRecord session, HostRecord? host) =>
    new(
        SessionId: session.SessionId,
        HostId: session.HostId,
        HostDisplayName: host?.DisplayName ?? session.HostId,
        RouteKind: session.RouteKind,
        NatStatus: session.NatStatus,
        ProbeEndpoint: session.ProbeEndpoint,
        HostNatProbe: session.HostNatProbe,
        ClientNatProbe: session.ClientNatProbe,
        UpdatedUtc: session.UpdatedUtc);

static string ComputeNatStatus(SessionRecord session)
{
    if (session.ProbeEndpoint is null || string.IsNullOrWhiteSpace(session.ProbeToken))
    {
        return "probe_unavailable";
    }

    if (session.HostNatProbe is null || session.ClientNatProbe is null)
    {
        return "probe_pending";
    }

    if (string.Equals(session.HostNatProbe.ObservedAddress, session.ClientNatProbe.ObservedAddress, StringComparison.OrdinalIgnoreCase))
    {
        return "same_public_ip";
    }

    return "punch_candidate";
}

static SessionRecord UpdateNatRouteAndStatus(
    SessionRecord session,
    string role,
    NatProbeObservation observation,
    DateTimeOffset now)
{
    var updated = session;

    if (string.Equals(role, "host", StringComparison.OrdinalIgnoreCase))
    {
        updated = updated with
        {
            HostNatProbe = observation,
            UpdatedUtc = now,
        };
    }
    else
    {
        updated = updated with
        {
            ClientNatProbe = observation,
            UpdatedUtc = now,
        };
    }

    updated = updated with
    {
        NatStatus = ComputeNatStatus(updated),
    };

    if (updated.HostNatProbe is not null &&
        updated.ClientNatProbe is not null &&
        string.Equals(updated.NatStatus, "same_public_ip", StringComparison.OrdinalIgnoreCase) &&
        updated.RouteKind is "direct_host_push" or "direct_fallback" or "relay_assigned")
    {
        updated = ApplyRoutePlanCore(
            updated,
            new SessionRoutePlan(
                RouteKind: "direct_punched",
                RelayId: null,
                RelayRegion: null,
                RelayEndpoint: null),
            now);
    }

    return updated;
}

static bool TryBuildDirectPunchedEndpoints(
    SessionRecord session,
    out StreamEndpoint? punchedStreamEndpoint,
    out StreamEndpoint? punchedReceiverEndpoint)
{
    punchedStreamEndpoint = null;
    punchedReceiverEndpoint = null;

    if (session.HostNatProbe is null ||
        session.ClientNatProbe is null ||
        string.IsNullOrWhiteSpace(session.HostNatProbe.ObservedAddress) ||
        string.IsNullOrWhiteSpace(session.ClientNatProbe.ObservedAddress) ||
        session.HostNatProbe.ObservedPort is < 1 or > 65535 ||
        session.ClientNatProbe.ObservedPort is < 1 or > 65535)
    {
        return false;
    }

    punchedStreamEndpoint = new StreamEndpoint(
        session.HostNatProbe.ObservedAddress,
        session.HostNatProbe.ObservedPort,
        "udp-evrt-direct-punch");
    punchedReceiverEndpoint = new StreamEndpoint(
        session.ClientNatProbe.ObservedAddress,
        session.ClientNatProbe.ObservedPort,
        "udp-evrt-direct-punch");
    return true;
}

static StreamEndpoint? BuildReceiverEndpoint(CreateSessionRequest request)
{
    if (string.IsNullOrWhiteSpace(request.ReceiverAddress))
    {
        return null;
    }

    return new StreamEndpoint(
        Host: request.ReceiverAddress.Trim(),
        Port: request.ReceiverPort is > 0 and <= 65535 ? request.ReceiverPort : 5001,
        Transport: "udp-evrt-direct");
}

static DesiredStreamSettings BuildDesiredStream(CreateSessionRequest request) =>
    new(
        RequestedWidth: request.RequestedWidth > 0 ? request.RequestedWidth : null,
        RequestedHeight: request.RequestedHeight > 0 ? request.RequestedHeight : null,
        RequestedFps: request.RequestedFps > 0 ? request.RequestedFps : null,
        RequestedBitrateBps: request.RequestedBitrateBps > 0 ? request.RequestedBitrateBps : null,
        CaptureCursor: request.CaptureCursor,
        AdaptiveMode: request.AdaptiveMode,
        PreferredCodecs: NormalizeList(request.PreferredCodecs, Array.Empty<string>()),
        PresetId: string.IsNullOrWhiteSpace(request.PresetId) ? null : request.PresetId.Trim());

static string GetStateSnapshotPath()
{
    var configuredPath = Environment.GetEnvironmentVariable("EVERTY_CONTROL_PLANE_STATE_PATH");
    if (!string.IsNullOrWhiteSpace(configuredPath))
    {
        return Path.GetFullPath(configuredPath);
    }

    var directory = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "Everty",
        "ControlPlane");
    return Path.Combine(directory, "state.json");
}

static PersistenceReadiness CheckPersistenceReadiness()
{
    var path = GetStateSnapshotPath();
    try
    {
        var directory = Path.GetDirectoryName(path);
        if (!string.IsNullOrWhiteSpace(directory))
        {
            Directory.CreateDirectory(directory);
        }

        var probePath = $"{path}.ready";
        File.WriteAllText(probePath, DateTimeOffset.UtcNow.ToString("O"));
        File.Delete(probePath);
        return new(path, Writable: true, Error: null);
    }
    catch (Exception ex)
    {
        return new(path, Writable: false, Error: ex.Message);
    }
}

static JsonSerializerOptions CreateStateSnapshotJsonOptions() =>
    new(JsonSerializerDefaults.Web)
    {
        WriteIndented = true,
    };

static void EnsureDemoUsers(ControlPlaneState state, ControlPlaneOptions options)
{
    if (!options.DemoAuthEnabled)
    {
        return;
    }

    var now = DateTimeOffset.UtcNow;
    lock (state.SyncRoot)
    {
        EnsureDemoUser(state, "admin", "admin", now);
        EnsureDemoUser(state, "test", "test", now);
    }
}

static void EnsureDemoUser(ControlPlaneState state, string email, string password, DateTimeOffset now)
{
    var normalizedEmail = NormalizeEmail(email);
    var existing = state.Users.Values.FirstOrDefault(user =>
        string.Equals(user.Email, normalizedEmail, StringComparison.OrdinalIgnoreCase));
    if (existing is not null)
    {
        if (!existing.Enabled)
        {
            state.Users[existing.UserId] = existing with
            {
                Enabled = true,
                UpdatedUtc = now,
            };
        }
        return;
    }

    var salt = CreateSecret();
    var user = new UserRecord(
        UserId: $"user_{Guid.NewGuid():N}",
        Email: normalizedEmail,
        PasswordSalt: salt,
        PasswordHash: HashPassword(salt, password),
        CreatedUtc: now,
        UpdatedUtc: now,
        LastSeenUtc: now,
        Enabled: true);
    state.Users[user.UserId] = user;
}

static void LoadStateSnapshot(ControlPlaneState state)
{
    var path = GetStateSnapshotPath();
    if (!File.Exists(path))
    {
        return;
    }

    var snapshot = JsonSerializer.Deserialize<ControlPlaneStateSnapshot>(
        File.ReadAllText(path),
        CreateStateSnapshotJsonOptions());
    if (snapshot is null)
    {
        return;
    }

    lock (state.SyncRoot)
    {
        state.Devices.Clear();
        foreach (var record in snapshot.Devices)
        {
            state.Devices[record.DeviceId] = record;
        }

        state.AccessTokens.Clear();
        foreach (var record in snapshot.AccessTokens)
        {
            state.AccessTokens[record.AccessToken] = record;
        }

        state.RefreshTokens.Clear();
        foreach (var record in snapshot.RefreshTokens)
        {
            state.RefreshTokens[record.RefreshToken] = record;
        }

        state.Users.Clear();
        foreach (var record in snapshot.Users)
        {
            state.Users[record.UserId] = record;
        }

        state.UserAccessTokens.Clear();
        foreach (var record in snapshot.UserAccessTokens)
        {
            state.UserAccessTokens[record.AccessToken] = record;
        }

        state.UserRefreshTokens.Clear();
        foreach (var record in snapshot.UserRefreshTokens)
        {
            state.UserRefreshTokens[record.RefreshToken] = record;
        }

        state.Relays.Clear();
        foreach (var record in snapshot.Relays)
        {
            state.Relays[record.RelayId] = record;
        }

        state.Hosts.Clear();
        foreach (var record in snapshot.Hosts)
        {
            state.Hosts[record.HostId] = record;
        }

        state.HostOffers.Clear();
        foreach (var record in snapshot.HostOffers ?? Array.Empty<HostOfferRecord>())
        {
            state.HostOffers[record.HostId] = record;
        }

        state.BillingAccounts.Clear();
        foreach (var record in snapshot.BillingAccounts ?? Array.Empty<BillingAccountRecord>())
        {
            state.BillingAccounts[record.HostId] = record;
        }

        state.BillingSessions.Clear();
        foreach (var record in snapshot.BillingSessions ?? Array.Empty<BillingSessionRecord>())
        {
            state.BillingSessions[record.SessionId] = record;
        }

        state.BillingLedger.Clear();
        state.BillingLedger.AddRange(snapshot.BillingLedger ?? Array.Empty<BillingLedgerEntryRecord>());

        state.Sessions.Clear();
        foreach (var record in snapshot.Sessions)
        {
            state.Sessions[record.SessionId] = record;
        }

        state.Telemetry.Clear();
        state.Telemetry.AddRange(snapshot.Telemetry);
    }
}

static void SaveStateSnapshot(ControlPlaneState state)
{
    var path = GetStateSnapshotPath();
    var directory = Path.GetDirectoryName(path);
    if (!string.IsNullOrWhiteSpace(directory))
    {
        Directory.CreateDirectory(directory);
    }

    var snapshot = new ControlPlaneStateSnapshot(
        Version: 1,
        SavedUtc: DateTimeOffset.UtcNow,
        Devices: state.Devices.Values.ToArray(),
        AccessTokens: state.AccessTokens.Values.ToArray(),
        RefreshTokens: state.RefreshTokens.Values.ToArray(),
        Users: state.Users.Values.ToArray(),
        UserAccessTokens: state.UserAccessTokens.Values.ToArray(),
        UserRefreshTokens: state.UserRefreshTokens.Values.ToArray(),
        Relays: state.Relays.Values.ToArray(),
        Hosts: state.Hosts.Values.ToArray(),
        HostOffers: state.HostOffers.Values.ToArray(),
        BillingAccounts: state.BillingAccounts.Values.ToArray(),
        BillingSessions: state.BillingSessions.Values.ToArray(),
        BillingLedger: state.BillingLedger.ToArray(),
        Sessions: state.Sessions.Values.ToArray(),
        Telemetry: state.Telemetry.ToArray());

    var tempPath = $"{path}.tmp";
    File.WriteAllText(tempPath, JsonSerializer.Serialize(snapshot, CreateStateSnapshotJsonOptions()));

    // File.Replace can fail on Windows when another process briefly holds the target.
    // Delete + move is more tolerant here, and we retry a few times before giving up.
    const int maxRetries = 3;
    for (var attempt = 0; attempt < maxRetries; attempt++)
    {
        try
        {
            if (File.Exists(path))
            {
                File.Delete(path);
            }

            File.Move(tempPath, path);
            return;
        }
        catch (IOException) when (attempt < maxRetries - 1)
        {
            Thread.Sleep(20 * (attempt + 1));
        }
    }

    if (File.Exists(path))
    {
        File.Delete(path);
    }
    File.Move(tempPath, path);
}

sealed class ControlPlaneState
{
    public object SyncRoot { get; } = new();
    public Dictionary<string, DeviceRecord> Devices { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, DeviceAccessTokenRecord> AccessTokens { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, DeviceRefreshTokenRecord> RefreshTokens { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, UserRecord> Users { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, UserAccessTokenRecord> UserAccessTokens { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, UserRefreshTokenRecord> UserRefreshTokens { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, RelayRecord> Relays { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, HostRecord> Hosts { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, HostOfferRecord> HostOffers { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, BillingAccountRecord> BillingAccounts { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, BillingSessionRecord> BillingSessions { get; } = new(StringComparer.Ordinal);
    public List<BillingLedgerEntryRecord> BillingLedger { get; } = new();
    public Dictionary<string, SessionRecord> Sessions { get; } = new(StringComparer.Ordinal);
    public List<TelemetryEventRecord> Telemetry { get; } = new();
}

sealed record ControlPlaneOptions(
    TimeSpan AccessTokenLifetime,
    TimeSpan RefreshTokenLifetime,
    long MaxRequestBodyBytes,
    string OperatorKey,
    bool DemoAuthEnabled,
    string PaymentProvider,
    string PaymentProviderEndpoint,
    string PaymentProviderApiKey)
{
    public bool OperatorAuthConfigured => !string.IsNullOrWhiteSpace(OperatorKey);
    public bool PaymentProviderEndpointConfigured => !string.IsNullOrWhiteSpace(PaymentProviderEndpoint);
    public string PaymentProviderMode => string.Equals(PaymentProvider, "manual", StringComparison.OrdinalIgnoreCase)
        ? "manual"
        : PaymentProviderEndpointConfigured
            ? "external_http"
            : "external_stub";
    public bool PaymentProviderConfigured => !string.IsNullOrWhiteSpace(PaymentProvider);

    public static ControlPlaneOptions Load()
    {
        return new ControlPlaneOptions(
            AccessTokenLifetime: TimeSpan.FromHours(ReadInt("EVERTY_CONTROL_PLANE_ACCESS_TOKEN_HOURS", 12, 1, 168)),
            RefreshTokenLifetime: TimeSpan.FromDays(ReadInt("EVERTY_CONTROL_PLANE_REFRESH_TOKEN_DAYS", 30, 1, 365)),
            MaxRequestBodyBytes: ReadLong("EVERTY_CONTROL_PLANE_MAX_REQUEST_BODY_BYTES", 1_048_576, 16_384, 16_777_216),
            OperatorKey: Environment.GetEnvironmentVariable("EVERTY_CONTROL_PLANE_OPERATOR_KEY")?.Trim() ?? string.Empty,
            DemoAuthEnabled: ReadBool("EVERTY_CONTROL_PLANE_DEMO_AUTH_ENABLED", true),
            PaymentProvider: NormalizePaymentProvider(Environment.GetEnvironmentVariable("EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER")),
            PaymentProviderEndpoint: NormalizeOptionalUri(Environment.GetEnvironmentVariable("EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT")),
            PaymentProviderApiKey: Environment.GetEnvironmentVariable("EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_API_KEY")?.Trim() ?? string.Empty);
    }

    private static int ReadInt(string name, int fallback, int min, int max)
    {
        var value = Environment.GetEnvironmentVariable(name);
        return int.TryParse(value, out var parsed) ? Math.Clamp(parsed, min, max) : fallback;
    }

    private static long ReadLong(string name, long fallback, long min, long max)
    {
        var value = Environment.GetEnvironmentVariable(name);
        return long.TryParse(value, out var parsed) ? Math.Clamp(parsed, min, max) : fallback;
    }

    private static bool ReadBool(string name, bool fallback)
    {
        var value = Environment.GetEnvironmentVariable(name);
        return bool.TryParse(value, out var parsed) ? parsed : fallback;
    }

    private static string NormalizePaymentProvider(string? value)
    {
        var provider = string.IsNullOrWhiteSpace(value) ? "manual" : value.Trim().ToLowerInvariant();
        return provider.All(character => char.IsLetterOrDigit(character) || character is '_' or '-') ? provider : "manual";
    }

    private static string NormalizeOptionalUri(string? value)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            return string.Empty;
        }

        var trimmed = value.Trim();
        return Uri.TryCreate(trimmed, UriKind.Absolute, out var uri) && uri.Scheme is "http" or "https"
            ? uri.ToString()
            : string.Empty;
    }
}

interface IPaymentProvider
{
    string Provider { get; }
    string Mode { get; }
    bool Configured { get; }
    bool EndpointConfigured { get; }
    bool ExternalCallsEnabled { get; }
    bool ManualCapture { get; }
    bool ManualSettlement { get; }

    PaymentProviderOperationResult ReserveHold(string sessionId, string hostId, decimal amount, string currency, DateTimeOffset now);
    PaymentProviderOperationResult Capture(string sessionId, string hostId, decimal amount, string currency, string? providerHoldId, DateTimeOffset now);
    PaymentProviderOperationResult Settle(string sessionId, string hostId, decimal amount, string currency, string? providerCaptureId, DateTimeOffset now);
}

sealed class PaymentProviderAdapter(ControlPlaneOptions options) : IPaymentProvider
{
    private static readonly HttpClient HttpClient = new()
    {
        Timeout = TimeSpan.FromSeconds(5),
    };

    public string Provider { get; } = options.PaymentProvider;
    public string Mode => options.PaymentProviderMode;
    public bool Configured => true;
    public bool EndpointConfigured => options.PaymentProviderEndpointConfigured;
    public bool ExternalCallsEnabled => Mode is "external_http";
    public bool ManualCapture => !ExternalCallsEnabled;
    public bool ManualSettlement => !ExternalCallsEnabled;

    public PaymentProviderOperationResult ReserveHold(string sessionId, string hostId, decimal amount, string currency, DateTimeOffset now) =>
        Execute("hold", sessionId, hostId, amount, currency, null, now);

    public PaymentProviderOperationResult Capture(string sessionId, string hostId, decimal amount, string currency, string? providerHoldId, DateTimeOffset now) =>
        Execute("capture", sessionId, hostId, amount, currency, providerHoldId, now);

    public PaymentProviderOperationResult Settle(string sessionId, string hostId, decimal amount, string currency, string? providerCaptureId, DateTimeOffset now) =>
        Execute("settle", sessionId, hostId, amount, currency, providerCaptureId, now);

    private PaymentProviderOperationResult Execute(string action, string sessionId, string hostId, decimal amount, string currency, string? previousProviderReferenceId, DateTimeOffset now)
    {
        if (Mode is "external_http")
        {
            return ExecuteHttp(action, sessionId, hostId, amount, currency, previousProviderReferenceId, now);
        }

        return CreateLocalResult(action, now);
    }

    private PaymentProviderOperationResult ExecuteHttp(string action, string sessionId, string hostId, decimal amount, string currency, string? previousProviderReferenceId, DateTimeOffset now)
    {
        var requestBody = new PaymentProviderOperationRequest(
            Provider: Provider,
            Action: action,
            SessionId: sessionId,
            HostId: hostId,
            Amount: amount,
            Currency: currency,
            PreviousProviderReferenceId: previousProviderReferenceId,
            RequestedUtc: now);

        using var request = new HttpRequestMessage(HttpMethod.Post, options.PaymentProviderEndpoint)
        {
            Content = new StringContent(JsonSerializer.Serialize(requestBody), Encoding.UTF8, "application/json"),
        };

        request.Headers.TryAddWithoutValidation("X-Everty-Payment-Provider", Provider);
        request.Headers.TryAddWithoutValidation("X-Everty-Payment-Action", action);
        if (!string.IsNullOrWhiteSpace(options.PaymentProviderApiKey))
        {
            request.Headers.TryAddWithoutValidation("Authorization", $"Bearer {options.PaymentProviderApiKey}");
        }

        using var response = HttpClient.Send(request);
        var responseBody = response.Content.ReadAsStringAsync().GetAwaiter().GetResult();
        if (!response.IsSuccessStatusCode)
        {
            throw new InvalidOperationException($"Payment provider {Provider} {action} failed with HTTP {(int)response.StatusCode}: {TrimForError(responseBody)}");
        }

        var providerResponse = JsonSerializer.Deserialize<PaymentProviderOperationResponse>(responseBody, CreatePaymentProviderJsonOptions());
        if (providerResponse is null || string.IsNullOrWhiteSpace(providerResponse.ProviderReferenceId))
        {
            throw new InvalidOperationException($"Payment provider {Provider} {action} did not return providerReferenceId.");
        }

        return new PaymentProviderOperationResult(
            ProviderReferenceId: NormalizeProviderReferenceId(providerResponse.ProviderReferenceId),
            Provider: string.IsNullOrWhiteSpace(providerResponse.Provider) ? Provider : NormalizeProviderReferenceId(providerResponse.Provider),
            RecordedUtc: providerResponse.RecordedUtc ?? now);
    }

    private PaymentProviderOperationResult CreateLocalResult(string action, DateTimeOffset now) =>
        new(ProviderReferenceId: $"{Provider}_{action}_{Guid.NewGuid():N}", Provider: Provider, RecordedUtc: now);

    private static JsonSerializerOptions CreatePaymentProviderJsonOptions() => new(JsonSerializerDefaults.Web);

    private static string NormalizeProviderReferenceId(string value)
    {
        var trimmed = value.Trim();
        return trimmed.Length <= 160 ? trimmed : trimmed[..160];
    }

    private static string TrimForError(string value)
    {
        var trimmed = value.Trim();
        return trimmed.Length <= 240 ? trimmed : trimmed[..240];
    }
}

sealed record PaymentProviderOperationResult(
    string ProviderReferenceId,
    string Provider,
    DateTimeOffset RecordedUtc);

sealed record PaymentProviderOperationRequest(
    string Provider,
    string Action,
    string SessionId,
    string HostId,
    decimal Amount,
    string Currency,
    string? PreviousProviderReferenceId,
    DateTimeOffset RequestedUtc);

sealed record PaymentProviderOperationResponse(
    string ProviderReferenceId,
    string? Provider,
    DateTimeOffset? RecordedUtc);

enum HostAvailability
{
    Offline,
    Online,
    Busy,
    Disabled,
}

enum RelayAvailability
{
    Offline,
    Online,
    Disabled,
}

enum SessionStatus
{
    Pending,
    Active,
    Stopped,
    Expired,
}

enum BillingStatus
{
    None,
    Held,
    Captured,
    Settled,
    Released,
    Failed,
}

sealed record DeviceLoginRequest(
    string? DeviceId,
    string? DeviceSecret,
    string DeviceLabel,
    string? Platform);

sealed record RefreshAccessTokenRequest(string RefreshToken);

sealed record RegisterUserRequest(
    string Email,
    string Password);

sealed record UserLoginRequest(
    string Email,
    string Password);

sealed record UserRefreshAccessTokenRequest(string RefreshToken);

sealed record AdminHostAvailabilityRequest(
    string Availability,
    string? Reason,
    bool StopActiveSession);

sealed record AdminRelayAvailabilityRequest(
    string Availability,
    string? Reason);

sealed record AdminStopSessionRequest(
    string? Reason);

sealed record AdminBillingSettleRequest(
    string? Reason);

sealed record AdminBillingRetryRequest(
    string? Action,
    string? Reason);

sealed record AdminHostOfferRequest(
    bool Listed,
    decimal PricePerHour,
    string? Currency,
    string? Description);

sealed record RegisterHostRequest(
    string? HostId,
    string? HostSecret,
    string DisplayName,
    string? Region,
    string? DirectAddress,
    int DirectPort,
    string[]? EncoderBackends,
    bool SupportsHevc,
    bool SupportsAudio,
    bool SupportsGamepad,
    HostCapabilitiesRequest? Capabilities);

sealed record HostCapabilitiesRequest(
    string? CpuModel = null,
    string? GpuModel = null,
    int RamGb = 0,
    int MaxWidth = 0,
    int MaxHeight = 0,
    int MaxFps = 0,
    string[]? SupportedEncodeCodecs = null,
    string[]? SupportedDecodeCodecs = null,
    string[]? SupportedEncoderBackends = null,
    string[]? LanAddresses = null);

sealed record ClientCapabilitiesRequest(
    string[]? SupportedDecodeCodecs = null,
    string[]? LanAddresses = null);

sealed record HostHeartbeatRequest(
    string HostSecret,
    double? CpuLoadPercent,
    double? GpuLoadPercent,
    double? NetworkKbps,
    HostAvailability? Availability,
    string? DirectAddress,
    int DirectPort);

sealed record RegisterRelayRequest(
    string? RelayId,
    string? RelaySecret,
    string DisplayName,
    string? Region,
    string PublicAddress,
    int UdpPort,
    RelayAvailability? Availability);

sealed record RelayHeartbeatRequest(
    string RelaySecret,
    RelayAvailability? Availability,
    string? PublicAddress,
    int UdpPort);

sealed record CreateSessionRequest(
    string HostId,
    string? ClientLabel,
    string? ClientRegion,
    string? CodecPreference,
    string[]? PreferredCodecs,
    string? PresetId,
    bool PreferRelay,
    bool ReplaceExistingActorSession,
    bool AudioRequested,
    int ControllerCount,
    int LeaseMinutes,
    string? ReceiverAddress,
    int ReceiverPort,
    int RequestedWidth,
    int RequestedHeight,
    int RequestedFps,
    int RequestedBitrateBps,
    bool? CaptureCursor,
    bool? AdaptiveMode,
    ClientCapabilitiesRequest? Capabilities);

sealed record SessionActionRequest(
    string SessionToken,
    string? Reason);

sealed record SessionNatProbeRequest(
    string SessionToken,
    string? ProbeToken,
    string Role,
    string ObservedAddress,
    int ObservedPort,
    string? LocalAddress,
    int? LocalPort,
    string? NetworkType);

sealed record SessionRelayRegistrationRequest(
    string SessionToken,
    string Role,
    string ObservedAddress,
    int ObservedPort);

sealed record TelemetryIngestRequest(
    string? HostId,
    string? SessionId,
    string? SessionToken,
    string? Source,
    string? EventType,
    Dictionary<string, object?>? Payload);

sealed record HostRecord(
    string HostId,
    string HostSecret,
    string DisplayName,
    string Region,
    string DirectAddress,
    int DirectPort,
    string[] EncoderBackends,
    bool SupportsHevc,
    bool SupportsAudio,
    bool SupportsGamepad,
    HostCapabilitiesRequest Capabilities,
    HostAvailability Availability,
    string? ActiveSessionId,
    DateTimeOffset LastSeenUtc,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc);

sealed record HostOfferRecord(
    string HostId,
    bool Listed,
    decimal PricePerHour,
    string Currency,
    string? Description,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc);

sealed record BillingAccountRecord(
    string HostId,
    string Currency,
    decimal Balance,
    decimal PendingAmount,
    decimal PlatformCommissionRate,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc);

sealed record BillingSessionRecord(
    string SessionId,
    string HostId,
    BillingStatus Status,
    decimal HoldAmount,
    decimal CapturedAmount,
    decimal SettledAmount,
    string Currency,
    decimal HourlyRate,
    decimal PlatformCommissionRate,
    string PaymentProvider,
    string? ProviderHoldId,
    string? ProviderCaptureId,
    string? ProviderSettlementId,
    string? LastPaymentError,
    DateTimeOffset? LastPaymentAttemptUtc,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc,
    DateTimeOffset? SettledUtc,
    string? Note);

sealed record BillingLedgerEntryRecord(
    string EntryId,
    string SessionId,
    string HostId,
    string Kind,
    decimal Amount,
    string Currency,
    DateTimeOffset RecordedUtc,
    string? Note);

sealed record RelayRecord(
    string RelayId,
    string RelaySecret,
    string DisplayName,
    string Region,
    string PublicAddress,
    int UdpPort,
    RelayAvailability Availability,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc,
    DateTimeOffset LastSeenUtc);

sealed record SessionRecord(
    string SessionId,
    string SessionToken,
    string HostId,
    string ClientLabel,
    string ClientRegion,
    string? CodecPreference,
    bool AudioRequested,
    int ControllerCount,
    StreamEndpoint StreamEndpoint,
    StreamEndpoint? ReceiverEndpoint,
    DesiredStreamSettings DesiredStream,
    string RouteKind,
    string RouteState,
    int RouteVersion,
    string? RelayId,
    string? RelayRegion,
    StreamEndpoint? RelayEndpoint,
    string? CreatedByDeviceId,
    string? CreatedByDeviceLabel,
    string? CreatedByUserId,
    string? CreatedByUserEmail,
    bool UnattendedAuthorized,
    string ProbeToken,
    StreamEndpoint? ProbeEndpoint,
    string NatStatus,
    NatProbeObservation? HostNatProbe,
    NatProbeObservation? ClientNatProbe,
    StreamEndpoint? ReceiverRegisteredEndpoint,
    DateTimeOffset? ReceiverRegisteredUtc,
    StreamEndpoint? SenderRegisteredEndpoint,
    DateTimeOffset? SenderRegisteredUtc,
    string? LastRouteActionKind,
    string? LastRouteActionReason,
    string? LastRouteActionActor,
    DateTimeOffset? LastRouteActionUtc,
    DateTimeOffset? RouteFallbackReadySinceUtc,
    DateTimeOffset? RouteRecoveryReadySinceUtc,
    int RouteRecoveryCount,
    DateTimeOffset? RouteRecoveryCooldownUntilUtc,
    int RouteFallbackCount,
    DateTimeOffset? RouteFallbackCooldownUntilUtc,
    int LeaseMinutes,
    SessionStatus Status,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc,
    DateTimeOffset ExpiresUtc,
    string? StopReason);

sealed record TelemetryEventRecord(
    string EventId,
    string EventType,
    string? HostId,
    string? SessionId,
    string Source,
    Dictionary<string, object?> Payload,
    DateTimeOffset RecordedUtc);

sealed record DeviceRecord(
    string DeviceId,
    string DeviceSecret,
    string DeviceLabel,
    string Platform,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc,
    DateTimeOffset LastSeenUtc);

sealed record DeviceAccessTokenRecord(
    string AccessToken,
    string DeviceId,
    DateTimeOffset ExpiresUtc,
    DateTimeOffset CreatedUtc);

sealed record DeviceRefreshTokenRecord(
    string RefreshToken,
    string DeviceId,
    DateTimeOffset ExpiresUtc,
    DateTimeOffset CreatedUtc);

sealed record UserRecord(
    string UserId,
    string Email,
    string PasswordSalt,
    string PasswordHash,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc,
    DateTimeOffset LastSeenUtc,
    bool Enabled);

sealed record UserAccessTokenRecord(
    string AccessToken,
    string UserId,
    DateTimeOffset ExpiresUtc,
    DateTimeOffset CreatedUtc);

sealed record UserRefreshTokenRecord(
    string RefreshToken,
    string UserId,
    DateTimeOffset ExpiresUtc,
    DateTimeOffset CreatedUtc);

sealed record ClientActor(
    string AuthKind,
    string? DeviceId,
    string? DeviceLabel,
    string? UserId,
    string? UserEmail,
    DateTimeOffset ExpiresUtc);

sealed record NatProbeObservation(
    string ObservedAddress,
    int ObservedPort,
    string? LocalAddress,
    int? LocalPort,
    string? NetworkType,
    DateTimeOffset ReportedUtc);

sealed record StreamEndpoint(
    string Host,
    int Port,
    string Transport);

sealed record ApiError(
    string Code,
    string Message);

sealed record HealthResponse(
    string Service,
    string BuildMarker,
    DateTimeOffset UtcNow,
    int RegisteredHosts,
    int OnlineHosts,
    int ActiveSessions,
    int TelemetryEvents);

sealed record ReadyResponse(
    string Service,
    string BuildMarker,
    DateTimeOffset UtcNow,
    bool Ready,
    string PersistencePath,
    bool PersistenceWritable,
    string? PersistenceError,
    int RegisteredHosts,
    int ActiveSessions);

sealed record RuntimeConfigResponse(
    string Service,
    string BuildMarker,
    int AccessTokenHours,
    int RefreshTokenDays,
    long MaxRequestBodyBytes,
    bool OperatorAuthConfigured,
    bool DemoAuthEnabled,
    string PaymentProvider,
    string PaymentProviderMode,
    bool PaymentProviderEndpointConfigured,
    bool SecurityHeadersEnabled,
    string PersistencePath);

sealed record AdminSummaryResponse(
    string Service,
    DateTimeOffset UtcNow,
    int RegisteredHosts,
    int OnlineHosts,
    int RegisteredRelays,
    int OnlineRelays,
    int Sessions,
    int ActiveSessions,
    int TelemetryEvents,
    int MarketplaceOffers,
    int ListedMarketplaceOffers,
    string PersistencePath,
    bool OperatorAuthConfigured);

sealed record BillingSummaryResponse(
    string Service,
    DateTimeOffset UtcNow,
    int TotalHolds,
    int PendingHolds,
    int CapturedHolds,
    int SettledHolds,
    decimal HeldAmount,
    decimal CapturedAmount,
    decimal SettledAmount,
    int LedgerEntries,
    int Accounts);

sealed record BillingProviderResponse(
    string Provider,
    string Mode,
    bool Configured,
    bool EndpointConfigured,
    bool ExternalCallsEnabled,
    bool ManualCapture,
    bool ManualSettlement);

sealed record BillingReconciliationItem(
    string SessionId,
    string HostId,
    BillingStatus BillingStatus,
    SessionStatus? SessionStatus,
    string PaymentProvider,
    string ActionRequired,
    decimal HoldAmount,
    decimal CapturedAmount,
    decimal SettledAmount,
    string Currency,
    string? ProviderHoldId,
    string? ProviderCaptureId,
    string? ProviderSettlementId,
    string? LastPaymentError,
    DateTimeOffset? LastPaymentAttemptUtc,
    DateTimeOffset UpdatedUtc);

sealed record BillingAccountSummary(
    string HostId,
    string Currency,
    decimal Balance,
    decimal PendingAmount,
    decimal PlatformCommissionRate,
    DateTimeOffset UpdatedUtc);

sealed record AdminSessionSummary(
    string SessionId,
    string HostId,
    string ClientLabel,
    string ClientRegion,
    SessionStatus Status,
    string RouteKind,
    string RouteState,
    int RouteVersion,
    string? RelayId,
    string CreatedByActor,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc,
    DateTimeOffset ExpiresUtc,
    string? StopReason,
    BillingStatus BillingStatus,
    decimal BillingHoldAmount,
    decimal BillingCapturedAmount,
    decimal BillingSettledAmount,
    string BillingCurrency);

sealed record BillingSessionDetails(
    string SessionId,
    string HostId,
    BillingStatus Status,
    decimal HoldAmount,
    decimal CapturedAmount,
    decimal SettledAmount,
    string Currency,
    decimal HourlyRate,
    decimal PlatformCommissionRate,
    string PaymentProvider,
    string? ProviderHoldId,
    string? ProviderCaptureId,
    string? ProviderSettlementId,
    string? LastPaymentError,
    DateTimeOffset? LastPaymentAttemptUtc,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc,
    DateTimeOffset? SettledUtc);

sealed record PersistenceReadiness(
    string Path,
    bool Writable,
    string? Error);

sealed record DeviceSummary(
    string DeviceId,
    string DeviceLabel,
    string Platform,
    DateTimeOffset CreatedUtc,
    DateTimeOffset LastSeenUtc);

sealed record UserSummary(
    string UserId,
    string Email,
    DateTimeOffset CreatedUtc,
    DateTimeOffset LastSeenUtc);

sealed record DeviceLoginResponse(
    string DeviceId,
    string DeviceSecret,
    string AccessToken,
    DateTimeOffset ExpiresUtc,
    string RefreshToken,
    DateTimeOffset RefreshExpiresUtc,
    DeviceSummary Device);

sealed record DeviceSessionResponse(
    DeviceSummary Device,
    DateTimeOffset ExpiresUtc);

sealed record RefreshAccessTokenResponse(
    string AccessToken,
    DateTimeOffset ExpiresUtc,
    string RefreshToken,
    DateTimeOffset RefreshExpiresUtc,
    DeviceSummary Device);

sealed record UserLoginResponse(
    string AccessToken,
    DateTimeOffset ExpiresUtc,
    string RefreshToken,
    DateTimeOffset RefreshExpiresUtc,
    UserSummary User);

sealed record UserSessionResponse(
    UserSummary User,
    DateTimeOffset ExpiresUtc);

sealed record RegisterHostResponse(
    string HostId,
    string HostSecret,
    int HeartbeatIntervalSeconds,
    StreamEndpoint StreamEndpoint,
    HostSummary Host);

sealed record RegisterRelayResponse(
    string RelayId,
    string RelaySecret,
    int HeartbeatIntervalSeconds,
    RelaySummary Relay);

sealed record HostHeartbeatResponse(
    string HostId,
    HostAvailability Availability,
    bool Online,
    string? ActiveSessionId,
    DateTimeOffset ServerUtc);

sealed record RelayHeartbeatResponse(
    string RelayId,
    RelayAvailability Availability,
    bool Online,
    DateTimeOffset ServerUtc);

sealed record HostSummary(
    string HostId,
    string HostCode,
    string DisplayName,
    string Region,
    bool Online,
    HostAvailability Availability,
    string? ActiveSessionId,
    StreamEndpoint StreamEndpoint,
    bool SupportsHevc,
    bool SupportsAudio,
    bool SupportsGamepad,
    IReadOnlyList<string> EncoderBackends,
    DateTimeOffset LastSeenUtc);

sealed record MarketplaceHostOfferResponse(
    string HostId,
    string DisplayName,
    string Region,
    bool Online,
    HostAvailability Availability,
    StreamEndpoint StreamEndpoint,
    bool SupportsHevc,
    bool SupportsAudio,
    bool SupportsGamepad,
    IReadOnlyList<string> EncoderBackends,
    decimal PricePerHour,
    string Currency,
    string? Description,
    DateTimeOffset UpdatedUtc);

sealed record HostDetails(
    HostSummary Summary,
    HostCapabilitiesRequest Capabilities,
    DateTimeOffset UpdatedUtc,
    DateTimeOffset CreatedUtc);

sealed record RelaySummary(
    string RelayId,
    string DisplayName,
    string Region,
    bool Online,
    RelayAvailability Availability,
    StreamEndpoint RelayEndpoint,
    int AssignedSessionCount,
    bool Saturated,
    DateTimeOffset LastSeenUtc);

sealed record TransportAnomaly(
    string Kind,
    string Reason,
    string Confidence);

sealed record SessionLeaseResponse(
    string SessionId,
    string SessionToken,
    string HostId,
    SessionStatus Status,
    StreamEndpoint StreamEndpoint,
    StreamEndpoint? ReceiverEndpoint,
    string RouteKind,
    string RouteState,
    int RouteVersion,
    string SessionHealth,
    string SessionHealthReason,
    string RouteActionHint,
    string RouteActionReason,
    int RouteFallbackReadyDurationSeconds,
    int RouteRecoveryReadyDurationSeconds,
    int RecommendedSyncDelaySeconds,
    string TransportLossLevel,
    string TransportAnomalyKind,
    string TransportAnomalyReason,
    string TransportAnomalyConfidence,
    int ReceiverTelemetryAgeSeconds,
    int SenderTelemetryAgeSeconds,
    string? LastRouteActionKind,
    string? LastRouteActionReason,
    string? LastRouteActionActor,
    DateTimeOffset? LastRouteActionUtc,
    int RouteRecoveryCount,
    int RouteRecoveryCooldownSeconds,
    int RouteFallbackCount,
    int RouteFallbackCooldownSeconds,
    StreamEndpoint? RelayEndpoint,
    string? RelayRegion,
    StreamEndpoint? ProbeEndpoint,
    string ProbeToken,
    string NatStatus,
    int HostNatProbeAgeSeconds,
    int ClientNatProbeAgeSeconds,
    bool NatProbeFresh,
    NatProbeObservation? HostNatProbe,
    NatProbeObservation? ClientNatProbe,
    DesiredStreamSettings DesiredStream,
    string? CodecPreference,
    bool AudioRequested,
    int ControllerCount,
    string HostDisplayName,
    DateTimeOffset ExpiresUtc);

sealed record SessionDetails(
    string SessionId,
    string HostId,
    string HostDisplayName,
    SessionStatus Status,
    StreamEndpoint StreamEndpoint,
    StreamEndpoint? ReceiverEndpoint,
    string RouteKind,
    string RouteState,
    int RouteVersion,
    string SessionHealth,
    string SessionHealthReason,
    string RouteActionHint,
    string RouteActionReason,
    int RouteFallbackReadyDurationSeconds,
    int RouteRecoveryReadyDurationSeconds,
    int RecommendedSyncDelaySeconds,
    string TransportLossLevel,
    string TransportAnomalyKind,
    string TransportAnomalyReason,
    string TransportAnomalyConfidence,
    int ReceiverTelemetryAgeSeconds,
    int SenderTelemetryAgeSeconds,
    string? LastRouteActionKind,
    string? LastRouteActionReason,
    string? LastRouteActionActor,
    DateTimeOffset? LastRouteActionUtc,
    int RouteRecoveryCount,
    int RouteRecoveryCooldownSeconds,
    int RouteFallbackCount,
    int RouteFallbackCooldownSeconds,
    StreamEndpoint? RelayEndpoint,
    string? RelayRegion,
    StreamEndpoint? ProbeEndpoint,
    string ProbeToken,
    string NatStatus,
    int HostNatProbeAgeSeconds,
    int ClientNatProbeAgeSeconds,
    bool NatProbeFresh,
    NatProbeObservation? HostNatProbe,
    NatProbeObservation? ClientNatProbe,
    DesiredStreamSettings DesiredStream,
    string? CodecPreference,
    bool AudioRequested,
    int ControllerCount,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc,
    DateTimeOffset ExpiresUtc,
    string? StopReason);

sealed record SessionConnectInstructions(
    string SessionId,
    string HostId,
    string HostDisplayName,
    SessionStatus Status,
    string RouteKind,
    string RouteState,
    int RouteVersion,
    string SessionHealth,
    string SessionHealthReason,
    string RouteActionHint,
    string RouteActionReason,
    int RouteFallbackReadyDurationSeconds,
    int RouteRecoveryReadyDurationSeconds,
    int RecommendedSyncDelaySeconds,
    string TransportLossLevel,
    string TransportAnomalyKind,
    string TransportAnomalyReason,
    string TransportAnomalyConfidence,
    int ReceiverTelemetryAgeSeconds,
    int SenderTelemetryAgeSeconds,
    string? LastRouteActionKind,
    string? LastRouteActionReason,
    string? LastRouteActionActor,
    DateTimeOffset? LastRouteActionUtc,
    int RouteRecoveryCount,
    int RouteRecoveryCooldownSeconds,
    int RouteFallbackCount,
    int RouteFallbackCooldownSeconds,
    StreamEndpoint StreamEndpoint,
    StreamEndpoint? ReceiverEndpoint,
    StreamEndpoint? RelayEndpoint,
    string? RelayRegion,
    StreamEndpoint? ProbeEndpoint,
    string ProbeToken,
    string NatStatus,
    bool ReceiverRegistered,
    bool HostReady,
    StreamEndpoint? ReceiverRegisteredEndpoint,
    StreamEndpoint? SenderRegisteredEndpoint,
    int HostNatProbeAgeSeconds,
    int ClientNatProbeAgeSeconds,
    bool NatProbeFresh,
    NatProbeObservation? HostNatProbe,
    NatProbeObservation? ClientNatProbe,
    DateTimeOffset ExpiresUtc);

sealed record SessionRoutePolicyResponse(
    string SessionId,
    string HostId,
    string RouteKind,
    string RouteState,
    int RouteVersion,
    string SessionHealth,
    string SessionHealthReason,
    string RouteActionHint,
    string RouteActionReason,
    int RecommendedSyncDelaySeconds,
    string TransportLossLevel,
    string TransportAnomalyKind,
    string TransportAnomalyReason,
    string TransportAnomalyConfidence,
    bool ActionableAnomaly,
    bool HighConfidenceAnomaly,
    int FallbackWarmupSeconds,
    int FallbackReadyDurationSeconds,
    bool FallbackReady,
    int RecoveryWarmupSeconds,
    int RecoveryReadyDurationSeconds,
    bool RecoveryReady,
    int FallbackCooldownSeconds,
    int RecoveryCooldownSeconds,
    int ReceiverTelemetryAgeSeconds,
    int SenderTelemetryAgeSeconds,
    string NatStatus,
    int HostNatProbeAgeSeconds,
    int ClientNatProbeAgeSeconds,
    bool NatProbeFresh);

sealed record HostLeaseResponse(
    string HostId,
    string SessionId,
    string SessionToken,
    string ClientLabel,
    SessionStatus Status,
    StreamEndpoint StreamEndpoint,
    StreamEndpoint? ReceiverEndpoint,
    string RouteKind,
    int RouteVersion,
    StreamEndpoint? RelayEndpoint,
    string? RelayRegion,
    StreamEndpoint? ProbeEndpoint,
    string ProbeToken,
    string NatStatus,
    bool ReceiverRegistered,
    bool HostReady,
    NatProbeObservation? HostNatProbe,
    NatProbeObservation? ClientNatProbe,
    DesiredStreamSettings DesiredStream,
    bool UnattendedAuthorized,
    string? CodecPreference,
    bool AudioRequested,
    int ControllerCount,
    DateTimeOffset CreatedUtc,
    DateTimeOffset UpdatedUtc,
    DateTimeOffset ExpiresUtc);

sealed record SessionNatStateResponse(
    string SessionId,
    string HostId,
    string HostDisplayName,
    string RouteKind,
    string NatStatus,
    StreamEndpoint? ProbeEndpoint,
    NatProbeObservation? HostNatProbe,
    NatProbeObservation? ClientNatProbe,
    DateTimeOffset UpdatedUtc);

sealed record DesiredStreamSettings(
    int? RequestedWidth,
    int? RequestedHeight,
    int? RequestedFps,
    int? RequestedBitrateBps,
    bool? CaptureCursor,
    bool? AdaptiveMode,
    IReadOnlyList<string> PreferredCodecs,
    string? PresetId);

sealed record SessionRoutePlan(
    string RouteKind,
    string? RelayId,
    string? RelayRegion,
    StreamEndpoint? RelayEndpoint);

sealed record ControlPlaneStateSnapshot(
    int Version,
    DateTimeOffset SavedUtc,
    DeviceRecord[] Devices,
    DeviceAccessTokenRecord[] AccessTokens,
    DeviceRefreshTokenRecord[] RefreshTokens,
    UserRecord[] Users,
    UserAccessTokenRecord[] UserAccessTokens,
    UserRefreshTokenRecord[] UserRefreshTokens,
    RelayRecord[] Relays,
    HostRecord[] Hosts,
    HostOfferRecord[]? HostOffers,
    BillingAccountRecord[]? BillingAccounts,
    BillingSessionRecord[]? BillingSessions,
    BillingLedgerEntryRecord[]? BillingLedger,
    SessionRecord[] Sessions,
    TelemetryEventRecord[] Telemetry);
