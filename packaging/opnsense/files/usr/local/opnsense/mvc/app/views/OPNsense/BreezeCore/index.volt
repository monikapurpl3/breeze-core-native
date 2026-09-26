{#
 # Three tabs:
 #  * Settings - enable, listen address, port and extra environment, in the
 #    OPNsense model (config.xml), plus a link straight to the panel;
 #  * Units - Breeze Core's own config.json, edited in place. The API key and
 #    V3 tokens and keys are never sent to this page: it is told only whether
 #    each is set, and a new one is sent only when one is typed;
 #  * Devices - enrolled clients, approving a pairing code, revoking.
 #
 # Everything that comes from config.json or from a client (unit names, device
 # labels a phone chose for itself) goes into the page with .text() or .val(),
 # never as HTML.
 #}
<script>
    $(document).ready(function() {
        var data_get_map = {'frm_general': "/api/breezecore/settings/get"};
        mapDataToFormUI(data_get_map).done(function(data) {
            formatTokenizersUI();
            $('.selectpicker').selectpicker('refresh');
            updatePanelLink();
        });

        // No markup needed for this: layouts/default.volt already carries
        // <li id="service_status_container"> beside the page title, and this
        // helper fills it with the start/restart/stop buttons. It calls
        // /api/breezecore/service/{status,start,restart,stop}.
        updateServiceControlUI('breezecore');

        $("#saveAct").click(function() {
            saveFormToEndpoint(url="/api/breezecore/settings/set", formid='frm_general', callback_ok=function() {
                $("#saveAct_progress").addClass("fa fa-spinner fa-pulse");
                ajaxCall(url="/api/breezecore/service/reconfigure", sendData={}, callback=function(data,status) {
                    $("#saveAct_progress").removeClass("fa fa-spinner fa-pulse");
                    // The buttons beside the title again, now that it has
                    // started or stopped.
                    updateServiceControlUI('breezecore');
                    updatePanelLink();
                });
            });
        });

        // Field ids are the form's ids with the dots escaped: model name, then
        // section, then field (see SettingsController).
        function updatePanelLink() {
            var host = $("#breezecore\\.general\\.listen").val();
            var port = $("#breezecore\\.general\\.port").val();
            // 0.0.0.0 is a bind address, not a destination -- send the admin to
            // the host they are already talking to.
            if (host === "0.0.0.0" || host === "::" || host === "" || host === undefined) {
                host = window.location.hostname;
            }
            var url = "http://" + host + ":" + (port || "8420") + "/";
            $("#panelLink").attr("href", url).text(url);
        }

        // OPNsense's framework HTML-escapes every string in an API response
        // (library/OPNsense/Mvc/Response.php), for pages that insert them as
        // HTML. This page inserts them as text instead, so it undoes that
        // once, here - otherwise a unit called "A & B" would show as
        // "A &amp; B", and a save would write that back into config.json.
        function plain(s) {
            return (s === null || s === undefined) ? "" : $("<textarea>").html(String(s)).text();
        }

        // One message box per tab: a list of problems, or one line of news.
        function say(box, kind, lines) {
            var el = $(box).removeClass("alert-danger alert-success alert-warning").addClass("alert-" + kind).empty();
            if (!lines || lines.length === 0) {
                el.hide();
                return;
            }
            if (lines.length === 1) {
                el.text(plain(lines[0]));
            } else {
                var ul = $("<ul>");
                $.each(lines, function(i, l) { ul.append($("<li>").text(plain(l))); });
                el.append(ul);
            }
            el.show();
        }

        // ------------------------------------------------------------ Units
        function field(cls, value, attrs) {
            return $('<input type="text" class="form-control">').addClass(cls).val(value).attr(attrs || {});
        }
        function secret(cls, placeholder) {
            // new-password: no browser should offer to autofill, or save, a
            // unit's credentials as if they were a login.
            return $('<input type="password" class="form-control" autocomplete="new-password">')
                .addClass(cls).attr("placeholder", placeholder);
        }
        function unitRow(u) {
            var tr = $("<tr>").data("orig", u ? plain(u.id) : "");
            var set = u && u.token_set && u.key_set;
            tr.append($("<td>").append(field("u-name", u ? plain(u.name) : "", {placeholder: "{{ lang._('Living room') }}"})));
            tr.append($("<td>").append(field("u-ip", u ? plain(u.ip) : "", {placeholder: "192.168.1.50"})));
            tr.append($("<td>").append(field("u-port", u ? u.port : 6444, {style: "width: 6em"})));
            tr.append($("<td>").append(field("u-id", u ? plain(u.id) : "", {placeholder: "{{ lang._('digits') }}"})));
            var cred = $("<td>");
            cred.append($('<div class="text-muted">').text(
                set ? "{{ lang._('V3 token and key: set') }}" :
                (u ? "{{ lang._('none: a V1/V2 unit') }}" : "{{ lang._('only for V3 units') }}")));
            cred.append(secret("u-token", set ? "{{ lang._('new token, or leave empty to keep it') }}" : "{{ lang._('token') }}"));
            cred.append(secret("u-key", set ? "{{ lang._('new key, or leave empty to keep it') }}" : "{{ lang._('key') }}"));
            if (set) {
                cred.append($("<label>").append($('<input type="checkbox" class="u-clear">'), " {{ lang._('remove the credentials') }}"));
            }
            tr.append(cred);
            tr.append($("<td>").append(
                $('<button type="button" class="btn btn-default btn-xs" title="{{ lang._('Remove this unit') }}"><span class="fa fa-trash fa-fw"></span></button>')
                    .click(function() { tr.remove(); })));
            return tr;
        }
        function loadUnits() {
            ajaxGet("/api/breezecore/units/get", {}, function(data, status) {
                var body = $("#unitRows").empty();
                if (!data || !data.units) {
                    say("#unitsMsg", "danger", [(data && data.message) || "{{ lang._('Could not read config.json.') }}"]);
                    return;
                }
                $.each(data.units, function(i, u) { body.append(unitRow(u)); });
                $("#apiKeyState").text(data.api_key_set ? "{{ lang._('set') }}" : "{{ lang._('not set - the server will not start without one') }}");
                hideKey();
                $("#showKey").toggle(!!data.api_key_set);
                if (!data.exists) {
                    say("#unitsMsg", "warning", ["{{ lang._('There is no config.json yet. Run breeze-core pair from a shell to discover and pair units, or add them here.') }}"]);
                }
            });
        }
        // A real form, so the credential fields sit where browsers expect
        // them - but never submitted: Enter in a field must not reload the page.
        $("#unitsForm").on("submit", function(e) { e.preventDefault(); });

        // The API key, only when asked for, and gone again on Hide, on a save
        // and on leaving the tab: it is the one secret this page will fetch
        // (pairing a client needs it), and it should not linger on screen.
        function hideKey() {
            $("#apiKeyShown").val("").hide();
            $("#showKey").text("{{ lang._('Show API key') }}").data("shown", false);
        }
        $("#showKey").click(function() {
            if ($(this).data("shown")) {
                hideKey();
                return;
            }
            ajaxCall("/api/breezecore/units/apikey", {}, function(data, status) {
                if (data && data.result === "ok") {
                    $("#apiKeyShown").val(plain(data.api_key)).show().select();
                    $("#showKey").text("{{ lang._('Hide') }}").data("shown", true);
                } else {
                    say("#unitsMsg", "danger", [(data && data.message) || "{{ lang._('Could not read the API key.') }}"]);
                }
            });
        });
        $('a[href="#units"]').on("hide.bs.tab", hideKey);
        $("#addUnit").click(function() { $("#unitRows").append(unitRow(null)); });
        $("#saveUnits").click(function() {
            var units = [];
            $("#unitRows tr").each(function() {
                var tr = $(this);
                units.push({
                    orig_id: String(tr.data("orig") || ""),
                    name: tr.find(".u-name").val(),
                    ip: tr.find(".u-ip").val(),
                    port: tr.find(".u-port").val(),
                    id: tr.find(".u-id").val(),
                    token: tr.find(".u-token").val(),
                    key: tr.find(".u-key").val(),
                    clear_credentials: tr.find(".u-clear").is(":checked")
                });
            });
            $("#saveUnits_progress").addClass("fa fa-spinner fa-pulse");
            ajaxCall("/api/breezecore/units/set", {units: units, api_key: $("#apiKeyNew").val()}, function(data, status) {
                $("#saveUnits_progress").removeClass("fa fa-spinner fa-pulse");
                if (data && data.result === "saved") {
                    $("#apiKeyNew").val("");
                    say("#unitsMsg", "success", [data.restarted
                        ? "{{ lang._('Saved, and Breeze Core was restarted to load it.') }}"
                        : "{{ lang._('Saved. Breeze Core was not running; it will load this when it starts.') }}"]);
                    loadUnits();
                    updateServiceControlUI('breezecore');
                } else {
                    say("#unitsMsg", "danger", (data && data.errors) || [(data && data.message) || "{{ lang._('Not saved.') }}"]);
                }
            });
        });

        // ---------------------------------------------------------- Devices
        function when(t) {
            return t ? new Date(t * 1000).toLocaleString() : "{{ lang._('never') }}";
        }
        function loadDevices() {
            ajaxGet("/api/breezecore/devices/list", {}, function(data, status) {
                var body = $("#deviceRows").empty();
                if (!data || data.result !== "ok") {
                    $("#deviceTable").hide();
                    say("#devicesMsg", "warning", [(data && data.message) || "{{ lang._('Could not list the devices.') }}"]);
                    return;
                }
                $("#deviceTable").show();
                if (data.devices.length === 0) {
                    body.append($("<tr>").append($('<td colspan="6" class="text-muted">').text("{{ lang._('No clients are enrolled yet.') }}")));
                }
                $.each(data.devices, function(i, d) {
                    var tr = $("<tr>");
                    tr.append($("<td>").text(plain(d.label) || "{{ lang._('(no label)') }}"));
                    tr.append($("<td>").text(d.auth_version === 2 ? "{{ lang._('Ed25519 key') }}" : "{{ lang._('bearer token') }}"));
                    tr.append($("<td>").text(when(d.created_at)));
                    tr.append($("<td>").text(when(d.last_used)));
                    tr.append($("<td>").text(when(d.expires_at)));
                    tr.append($("<td>").append(
                        $('<button type="button" class="btn btn-default btn-xs"></button>')
                            .text("{{ lang._('Revoke') }}")
                            .click(function() {
                                if (!confirm("{{ lang._('Revoke this client? It will have to pair again.') }}")) {
                                    return;
                                }
                                ajaxCall("/api/breezecore/devices/revoke", {token_id: plain(d.token_id)}, function(r) {
                                    say("#devicesMsg", r && r.result === "revoked" ? "success" : "danger",
                                        [r && r.result === "revoked" ? "{{ lang._('Revoked.') }}" : ((r && r.message) || "{{ lang._('Not revoked.') }}")]);
                                    loadDevices();
                                });
                            })));
                    body.append(tr);
                });
            });
        }
        $("#approveAct").click(function() {
            ajaxCall("/api/breezecore/devices/approve", {code: $("#approveCode").val()}, function(data, status) {
                if (data && data.result === "approved") {
                    $("#approveCode").val("");
                    say("#devicesMsg", "success", ["{{ lang._('Approved') }}" + (data.label ? ": " + plain(data.label) : ".")]);
                    loadDevices();
                } else {
                    say("#devicesMsg", "danger", [(data && data.message) || "{{ lang._('Not approved.') }}"]);
                }
            });
        });
        $("#approveCode").keypress(function(e) { if (e.which === 13) { $("#approveAct").click(); } });
        $("#refreshDevices").click(loadDevices);

        // Load a tab when it is opened, so the Devices list is never stale.
        $('a[href="#units"]').on("shown.bs.tab", loadUnits);
        $('a[href="#devices"]').on("shown.bs.tab", loadDevices);
    });
</script>

<ul class="nav nav-tabs" data-tabs="tabs" id="maintabs">
    <li class="active"><a data-toggle="tab" href="#settings">{{ lang._('Settings') }}</a></li>
    <li><a data-toggle="tab" href="#units">{{ lang._('Units') }}</a></li>
    <li><a data-toggle="tab" href="#devices">{{ lang._('Devices') }}</a></li>
</ul>

<div class="tab-content content-box">
    <div id="settings" class="tab-pane fade in active">
        {{ partial("layout_partials/base_form", ['fields': formGeneral, 'id': 'frm_general']) }}
        <div class="col-md-12">
            <hr/>
            <button class="btn btn-primary" id="saveAct" type="button">
                <b>{{ lang._('Save') }}</b> <i id="saveAct_progress"></i>
            </button>
        </div>
        <div class="col-md-12" style="padding-top: 1.5em; padding-bottom: 1.5em;">
            <h2>{{ lang._('Web panel') }}</h2>
            <p>
                {{ lang._('Once enabled, the panel is at') }}
                <a id="panelLink" href="#" target="_blank">...</a>
            </p>
            <p>
                {{ lang._('Pairing a phone or browser needs approval from the local network -- that is deliberate, not a fault. Approve it on the Devices tab, or from a shell on this firewall:') }}
            </p>
            <pre>breeze-core devices           # what is already enrolled
breeze-core approve &lt;CODE&gt;</pre>
            <p>{{ lang._('Air conditioners are discovered and paired with:') }}</p>
            <pre>breeze-core pair</pre>
        </div>
    </div>

    <div id="units" class="tab-pane fade in" style="padding: 1.5em;">
        <p>
            {{ lang._("The units in Breeze Core's own config.json, edited in place on this firewall. It is not copied into the firewall's configuration or its backups, and V3 credentials are never sent to this page: it only shows whether each is set. The API key is sent only when you press Show API key. Saving restarts Breeze Core if it is running.") }}
        </p>
        <div class="alert" id="unitsMsg" style="display: none;"></div>
        <form id="unitsForm" autocomplete="off">
        <table class="table table-condensed">
            <tr>
                <td style="width: 12em;"><b>{{ lang._('API key') }}</b></td>
                <td>
                    <span id="apiKeyState">...</span>
                    <button class="btn btn-default btn-xs" id="showKey" type="button" style="display: none; margin-left: 1em;">{{ lang._('Show API key') }}</button>
                    <input type="text" class="form-control" id="apiKeyShown" readonly="readonly" autocomplete="off"
                           style="display: none; max-width: 30em; font-family: monospace;"/>
                    <div class="text-muted">{{ lang._('The app and the panel ask for this key when they pair. Having it lets a client ask to pair; only an approval on the Devices tab lets it in.') }}</div>
                    <input type="password" class="form-control" id="apiKeyNew" autocomplete="new-password"
                           placeholder="{{ lang._('a new key, or leave empty to keep it') }}" style="max-width: 30em;"/>
                    <div class="text-muted">{{ lang._('Replacing it means every app and browser needs the new key.') }}</div>
                </td>
            </tr>
        </table>
        <table class="table table-striped table-condensed">
            <thead>
                <tr>
                    <th>{{ lang._('Name') }}</th>
                    <th>{{ lang._('IP address') }}</th>
                    <th>{{ lang._('Port') }}</th>
                    <th>{{ lang._('Unit id') }}</th>
                    <th>{{ lang._('V3 credentials') }}</th>
                    <th></th>
                </tr>
            </thead>
            <tbody id="unitRows"></tbody>
        </table>
        <button class="btn btn-default" id="addUnit" type="button"><span class="fa fa-plus fa-fw"></span> {{ lang._('Add a unit') }}</button>
        <button class="btn btn-primary" id="saveUnits" type="button"><b>{{ lang._('Save units') }}</b> <i id="saveUnits_progress"></i></button>
        </form>
        <p class="text-muted" style="padding-top: 1em;">
            {{ lang._('New units are easiest to add with breeze-core pair, which finds them on the network and fetches V3 credentials from the cloud.') }}
        </p>
    </div>

    <div id="devices" class="tab-pane fade in" style="padding: 1.5em;">
        <p>{{ lang._('Phones and browsers enrolled with this server. Approving here does what breeze-core approve does in a shell.') }}</p>
        <div class="alert" id="devicesMsg" style="display: none;"></div>
        <div class="form-inline" style="padding-bottom: 1em;">
            <input type="text" class="form-control" id="approveCode" autocomplete="off"
                   placeholder="{{ lang._('pairing code') }}" style="text-transform: uppercase;"/>
            <button class="btn btn-primary" id="approveAct" type="button">{{ lang._('Approve') }}</button>
            <button class="btn btn-default" id="refreshDevices" type="button"><span class="fa fa-refresh fa-fw"></span> {{ lang._('Refresh') }}</button>
        </div>
        <table class="table table-striped table-condensed" id="deviceTable">
            <thead>
                <tr>
                    <th>{{ lang._('Client') }}</th>
                    <th>{{ lang._('Credential') }}</th>
                    <th>{{ lang._('Enrolled') }}</th>
                    <th>{{ lang._('Last used') }}</th>
                    <th>{{ lang._('Expires') }}</th>
                    <th></th>
                </tr>
            </thead>
            <tbody id="deviceRows"></tbody>
        </table>
    </div>
</div>
