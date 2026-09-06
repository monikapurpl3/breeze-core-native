<?php

namespace OPNsense\BreezeCore;

use OPNsense\Base\BaseModel;

/**
 * Breeze Core settings.
 *
 * Deliberately three fields. The API key and the paired units belong to Breeze
 * Core's own config.json -- managed by its panel and its LAN-approved pairing
 * flow -- and must not be mirrored here, or a GUI save would overwrite
 * credentials.
 */
class BreezeCore extends BaseModel
{
}
