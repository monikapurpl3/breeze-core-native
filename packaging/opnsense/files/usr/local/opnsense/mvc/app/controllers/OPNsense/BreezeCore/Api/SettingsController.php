<?php

namespace OPNsense\BreezeCore\Api;

use OPNsense\Base\ApiMutableModelControllerBase;

/**
 * get/set against the BreezeCore model. Validation comes from BreezeCore.xml for
 * free, so the form cannot save port 70000 or a malformed address.
 *
 * $internalModelName names the WHOLE model, not a section of it: getAction()
 * returns [name => every node] and setAction() applies POST[name] at the
 * model's root. So the form's ids are breezecore.general.*. In 4.1.1 this was
 * 'general' with ids general.*, which made every save post {enabled, listen,
 * port} at the root, where no such nodes exist: nothing was set, the save
 * reported success, and the service stayed disabled on 127.0.0.1:8420. Found
 * on a real OPNsense 26.7.
 */
class SettingsController extends ApiMutableModelControllerBase
{
    protected static $internalModelName = 'breezecore';
    protected static $internalModelClass = 'OPNsense\BreezeCore\BreezeCore';
}
