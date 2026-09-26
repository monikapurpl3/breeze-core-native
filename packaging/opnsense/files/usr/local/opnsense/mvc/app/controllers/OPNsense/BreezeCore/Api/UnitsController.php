<?php

namespace OPNsense\BreezeCore\Api;

use OPNsense\Base\ApiControllerBase;
use OPNsense\Base\UserException;
use OPNsense\Core\Backend;

/**
 * The units - and the API key - in Breeze Core's own config.json, edited in
 * place on the firewall.
 *
 * Never through the model: config.json holds the API key and each V3 unit's
 * token and key, which Midea will not issue again, and anything in the model
 * lands in config.xml - and so in every config backup, HA sync and cloud
 * backup of this firewall.
 *
 * And not back to the browser: get says whether each secret is set, never
 * what it is. set takes a new one only when one is typed, keeps the old one
 * otherwise, and clears V3 credentials only when asked to. The one exception
 * is the API key, which apikeyAction returns on an explicit click, because
 * pairing a client needs it; V3 credentials never leave the firewall.
 *
 * The server keeps config.json in memory and writes it itself (its own panel
 * renames units), so a save here stops the service, writes the file, and
 * starts it again if it was running - otherwise the two would overwrite each
 * other, and the server would not see the change until its next restart.
 */
class UnitsController extends ApiControllerBase
{
    const CONFIG = '/usr/local/etc/breeze-core/config.json';
    const DEFAULT_UNIT_PORT = 6444;

    public static function readConfig(): array
    {
        if (!file_exists(self::CONFIG)) {
            return [];
        }
        $doc = json_decode((string)file_get_contents(self::CONFIG), true);
        if (!is_array($doc)) {
            throw new UserException(
                gettext('config.json is not valid JSON, so it is not changed from here. Fix it from a shell first.'),
                'Breeze Core'
            );
        }
        return $doc;
    }

    public function getAction()
    {
        $doc = self::readConfig();
        $units = [];
        foreach ((array)($doc['units'] ?? []) as $u) {
            $units[] = [
                'id' => (string)($u['id'] ?? ''),
                'name' => (string)($u['name'] ?? ''),
                'ip' => (string)($u['ip'] ?? ''),
                'port' => (int)($u['port'] ?? self::DEFAULT_UNIT_PORT),
                'token_set' => !empty($u['token']),
                'key_set' => !empty($u['key']),
            ];
        }
        return [
            'exists' => file_exists(self::CONFIG),
            'api_key_set' => !empty($doc['api_key']),
            'units' => $units,
        ];
    }

    /**
     * The API key itself, for the page's "Show API key" button - the one
     * secret the page can ask for, because a new phone or browser cannot be
     * paired without it, and without this the only way to read it would be a
     * shell. Only on an explicit click, only by POST (so nothing prefetches or
     * caches it), and only for full admins. V3 tokens and keys are never
     * returned: nothing on the page needs them back.
     */
    public function apikeyAction()
    {
        if (!$this->request->isPost()) {
            return ['result' => 'failed'];
        }
        $this->throwNotFullAdmin();
        $doc = self::readConfig();
        if (empty($doc['api_key'])) {
            return ['result' => 'failed', 'message' => gettext('config.json has no API key yet.')];
        }
        return ['result' => 'ok', 'api_key' => (string)$doc['api_key']];
    }

    public function setAction()
    {
        if (!$this->request->isPost()) {
            return ['result' => 'failed'];
        }
        // Credentials that cannot be issued again: read-only users are out,
        // and so is anyone without full admin, as with other sensitive pages.
        $this->throwReadOnly();
        $this->throwNotFullAdmin();

        $posted = $this->request->getPost('units');
        $posted = is_array($posted) ? $posted : [];
        $newApiKey = trim((string)$this->request->getPost('api_key'));

        $doc = self::readConfig();
        $existing = [];
        foreach ((array)($doc['units'] ?? []) as $u) {
            $existing[(string)($u['id'] ?? '')] = $u;
        }

        $errors = [];
        $units = [];
        $ids = [];
        $names = [];
        foreach (array_values($posted) as $i => $p) {
            $row = $i + 1;
            $name = trim((string)($p['name'] ?? ''));
            $ip = trim((string)($p['ip'] ?? ''));
            $port = trim((string)($p['port'] ?? ''));
            $id = trim((string)($p['id'] ?? ''));
            $origId = (string)($p['orig_id'] ?? '');
            $token = trim((string)($p['token'] ?? ''));
            $key = trim((string)($p['key'] ?? ''));
            $clear = !empty($p['clear_credentials']) && $p['clear_credentials'] !== 'false';

            if ($name === '' || strlen($name) > 64 || preg_match('/[\x00-\x1f\x7f]/', $name)) {
                $errors[] = sprintf(gettext('Unit %d: a name of 1 to 64 printable characters.'), $row);
            } elseif (isset($names[strtolower($name)])) {
                $errors[] = sprintf(gettext('Unit %d: another unit is already called "%s".'), $row, $name);
            }
            $names[strtolower($name)] = true;
            if (filter_var($ip, FILTER_VALIDATE_IP) === false) {
                $errors[] = sprintf(gettext('Unit %d: "%s" is not an IP address.'), $row, $ip);
            }
            if ($port === '') {
                $port = (string)self::DEFAULT_UNIT_PORT;
            }
            if (!ctype_digit($port) || (int)$port < 1 || (int)$port > 65535) {
                $errors[] = sprintf(gettext('Unit %d: the port must be 1 to 65535.'), $row);
            }
            // Midea ids are 48-bit; anything past PHP_INT_MAX is not one.
            if (!ctype_digit($id) || strlen($id) > 18) {
                $errors[] = sprintf(gettext('Unit %d: the id is the unit\'s number, digits only.'), $row);
            } elseif (isset($ids[$id])) {
                $errors[] = sprintf(gettext('Unit %d: another unit already has id %s.'), $row, $id);
            }
            $ids[$id] = true;
            foreach (['token' => $token, 'key' => $key] as $what => $value) {
                if ($value !== '' && !preg_match('/^([0-9A-Fa-f]{2})+$/', $value)) {
                    $errors[] = sprintf(gettext('Unit %d: the V3 %s is hexadecimal.'), $row, $what);
                }
            }

            // Start from what is there, so fields this page does not know
            // about survive a save.
            $unit = ($origId !== '' && isset($existing[$origId])) ? $existing[$origId] : [];
            $unit['name'] = $name;
            $unit['ip'] = $ip;
            $unit['port'] = (int)$port;
            $unit['id'] = (int)$id;
            if ($clear) {
                $unit['token'] = null;
                $unit['key'] = null;
            }
            if ($token !== '') {
                $unit['token'] = $token;
            }
            if ($key !== '') {
                $unit['key'] = $key;
            }
            $unit += ['token' => null, 'key' => null];
            // A V3 unit needs both; one without the other can never connect.
            if (empty($unit['token']) !== empty($unit['key'])) {
                $errors[] = sprintf(gettext('Unit %d: a V3 unit needs both a token and a key, or neither.'), $row);
            }
            $units[] = $unit;
        }
        if ($newApiKey !== '' && (strlen($newApiKey) < 16 || preg_match('/\s/', $newApiKey))) {
            $errors[] = gettext('The API key must be at least 16 characters, with no spaces.');
        }
        if (!empty($errors)) {
            return ['result' => 'failed', 'errors' => $errors];
        }

        $doc['units'] = $units;
        if ($newApiKey !== '') {
            $doc['api_key'] = $newApiKey;
        }
        $doc += ['api_key' => null];

        $backend = new Backend();
        $wasRunning = strpos($backend->configdRun('breezecore status'), 'is running') !== false;
        if ($wasRunning) {
            $backend->configdRun('breezecore stop');
        }
        $this->writeConfig($doc);
        if ($wasRunning) {
            $backend->configdRun('breezecore start');
        }
        return ['result' => 'saved', 'restarted' => $wasRunning];
    }

    /**
     * Beside the old file, then renamed over it, so a failed write never
     * leaves half a config.json; owned by the service account, mode 640, as
     * the server itself writes it.
     */
    private function writeConfig(array $doc): void
    {
        $dir = dirname(self::CONFIG);
        if (!is_dir($dir)) {
            mkdir($dir, 0750, true);
            chown($dir, 'breeze');
            chgrp($dir, 'breeze');
        }
        $json = json_encode($doc, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE) . "\n";
        $tmp = self::CONFIG . '.tmp-' . bin2hex(random_bytes(4));
        $old = umask(0137);
        $ok = file_put_contents($tmp, $json, LOCK_EX);
        umask($old);
        if ($ok === false) {
            throw new UserException(gettext('Could not write config.json.'), 'Breeze Core');
        }
        chmod($tmp, 0640);
        chown($tmp, 'breeze');
        chgrp($tmp, 'breeze');
        if (!rename($tmp, self::CONFIG)) {
            @unlink($tmp);
            throw new UserException(gettext('Could not replace config.json.'), 'Breeze Core');
        }
    }
}
