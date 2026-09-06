<?php

namespace OPNsense\BreezeCore\Api;

use OPNsense\Base\ApiMutableServiceControllerBase;

/**
 * start / stop / restart / status, routed through configd rather than executed
 * from PHP -- the OPNsense convention, and the reason the GUI process needs no
 * privileges of its own.
 */
class ServiceController extends ApiMutableServiceControllerBase
{
    protected static $internalServiceClass = 'OPNsense\BreezeCore\BreezeCore';
    protected static $internalServiceTemplate = 'OPNsense/BreezeCore';
    protected static $internalServiceEnabled = 'general.enabled';
    protected static $internalServiceName = 'breezecore';
}
