<?php

namespace OPNsense\BreezeCore\Api;

use OPNsense\Base\ApiMutableModelControllerBase;

/**
 * get/set against the BreezeCore model. Validation comes from BreezeCore.xml for
 * free, so the form cannot save port 70000 or a malformed address.
 */
class SettingsController extends ApiMutableModelControllerBase
{
    protected static $internalModelName = 'general';
    protected static $internalModelClass = 'OPNsense\BreezeCore\BreezeCore';
}
