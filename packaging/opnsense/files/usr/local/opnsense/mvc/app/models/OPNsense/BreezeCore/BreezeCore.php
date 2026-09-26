<?php

namespace OPNsense\BreezeCore;

use OPNsense\Base\BaseModel;
use OPNsense\Base\Messages\Message;

/**
 * Breeze Core settings: enable, listen address, port and extra environment.
 *
 * The API key and the paired units are NOT here, and must never be. They live
 * in Breeze Core's own config.json, edited in place by Api/UnitsController.
 * Everything in this model lands in config.xml, which OPNsense copies into its
 * config backups, its HA sync and any cloud backup - the last places a V3
 * unit's token and key, which Midea will not issue again, belong.
 */
class BreezeCore extends BaseModel
{
    /**
     * Names the plugin sets itself, from the port and listen address or from
     * where config.json lives. A second copy in the free-form list would at
     * best be ignored (serve.sh skips them too) and at worst quietly undo one
     * of those choices.
     */
    const MANAGED_ENV = [
        'AC_CONFIG', 'AC_CONFIG_DIR', 'AC_DEVICES', 'AC_PROGRAMS', 'AC_TIMERS',
        'AC_BEHIND_PROXY', 'BREEZE_HOST', 'BREEZE_PORT',
    ];

    /**
     * What is wrong with an environment list, one message per bad line.
     * Blank lines and lines starting with # are allowed, so examples can stay
     * in the box commented out.
     */
    public static function environmentProblems(string $text): array
    {
        $problems = [];
        foreach (preg_split('/\r\n|\r|\n/', $text) as $i => $raw) {
            $line = trim($raw);
            if ($line === '' || $line[0] === '#') {
                continue;
            }
            if (!preg_match('/^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/', $line, $m)) {
                $problems[] = sprintf(
                    gettext('Line %d is not NAME=value (a name is letters, digits and _, not starting with a digit): %s'),
                    $i + 1,
                    $line
                );
                continue;
            }
            if (in_array(strtoupper($m[1]), self::MANAGED_ENV, true)) {
                $problems[] = sprintf(
                    gettext('Line %d: %s is set from the listen address, the port or the data folder, so it cannot be set here.'),
                    $i + 1,
                    $m[1]
                );
            }
        }
        return $problems;
    }

    /**
     * {@inheritdoc}
     */
    public function performValidation($validateFullModel = false)
    {
        $messages = parent::performValidation($validateFullModel);
        $node = $this->general->environment;
        if ($validateFullModel || $node->isFieldChanged()) {
            foreach (self::environmentProblems((string)$node) as $problem) {
                $messages->appendMessage(new Message($problem, 'general.environment'));
            }
        }
        return $messages;
    }
}
