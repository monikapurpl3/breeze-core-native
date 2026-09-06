{#
 # Two things beyond the usual save/apply form:
 #  * a link straight to the panel, because "where is it now" is the first
 #    question after enabling it;
 #  * a plain statement that pairing is approved on the LAN, since that is the
 #    step people get stuck on and it is deliberate rather than a fault.
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
                    // No markup needed for this: layouts/default.volt already carries
        // <li id="service_status_container"> beside the page title, and this
        // helper fills it with the start/restart/stop buttons. It calls
        // /api/breezecore/service/{status,start,restart,stop}.
        updateServiceControlUI('breezecore');
                    updatePanelLink();
                });
            });
        });

        function updatePanelLink() {
            var host = $("#general\\.listen").val();
            var port = $("#general\\.port").val();
            // 0.0.0.0 is a bind address, not a destination -- send the admin to
            // the host they are already talking to.
            if (host === "0.0.0.0" || host === "::" || host === "" || host === undefined) {
                host = window.location.hostname;
            }
            var url = "http://" + host + ":" + (port || "8420") + "/";
            $("#panelLink").attr("href", url).text(url);
        }
    });
</script>

<div class="content-box" style="padding-bottom: 1.5em;">
    {{ partial("layout_partials/base_form", ['fields': formGeneral, 'id': 'frm_general']) }}
    <div class="col-md-12">
        <hr/>
        <button class="btn btn-primary" id="saveAct" type="button">
            <b>{{ lang._('Save') }}</b> <i id="saveAct_progress"></i>
        </button>
    </div>
</div>

<div class="content-box" style="padding: 1.5em;">
    <h2>{{ lang._('Web panel') }}</h2>
    <p>
        {{ lang._('Once enabled, the panel is at') }}
        <a id="panelLink" href="#" target="_blank">...</a>
    </p>
    <p>
        {{ lang._('Pairing a phone or browser needs approval from the local network -- that is deliberate, not a fault. From a shell on this firewall:') }}
    </p>
    <pre>breeze-core devices           # what is already enrolled
breeze-core approve &lt;CODE&gt;</pre>
    <p>{{ lang._('Air conditioners are discovered and paired with:') }}</p>
    <pre>breeze-core pair</pre>
</div>
