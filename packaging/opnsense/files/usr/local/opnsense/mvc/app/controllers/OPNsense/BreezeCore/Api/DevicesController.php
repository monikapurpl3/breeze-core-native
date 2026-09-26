<?php

namespace OPNsense\BreezeCore\Api;

use OPNsense\Base\ApiControllerBase;
use OPNsense\BreezeCore\BreezeCore;
use OPNsense\Core\Backend;

/**
 * Enrolled clients: list, approve a pairing code, revoke.
 *
 * Through the running server's own admin API, exactly as `breeze-core devices`,
 * `approve` and `revoke` do: the API key from config.json, from this machine,
 * which is on the private network the server insists on for these. Pending
 * codes live only in the server's memory, so there is no other way to approve
 * one - and it keeps devices.json written by the one process that owns it.
 *
 * The key goes from PHP to the server on this machine and never to the
 * browser.
 */
class DevicesController extends ApiControllerBase
{
    /** [base url, api key], or an error message for the page. */
    private function server()
    {
        if (strpos((new Backend())->configdRun('breezecore status'), 'is running') === false) {
            return gettext('Breeze Core is not running. Enable it on the Settings tab, or start it, to manage devices.');
        }
        $doc = UnitsController::readConfig();
        if (empty($doc['api_key'])) {
            return gettext('config.json has no API key yet.');
        }
        $mdl = new BreezeCore();
        $host = (string)$mdl->general->listen;
        $port = (int)(string)$mdl->general->port;
        // A wildcard bind is reachable on loopback; a specific one only on
        // that address.
        if ($host === '' || $host === '0.0.0.0') {
            $host = '127.0.0.1';
        } elseif ($host === '::') {
            $host = '::1';
        }
        if (strpos($host, ':') !== false) {
            $host = '[' . $host . ']';
        }
        return ["http://{$host}:{$port}", (string)$doc['api_key']];
    }

    /** [status, decoded body], or an error message. */
    private function call($method, $path, $body = null)
    {
        $server = $this->server();
        if (is_string($server)) {
            return $server;
        }
        [$base, $key] = $server;
        $ch = curl_init($base . $path);
        curl_setopt_array($ch, [
            CURLOPT_CUSTOMREQUEST => $method,
            CURLOPT_RETURNTRANSFER => true,
            CURLOPT_CONNECTTIMEOUT => 3,
            CURLOPT_TIMEOUT => 8,
            CURLOPT_HTTPHEADER => ['X-API-Key: ' . $key, 'Content-Type: application/json', 'Accept: application/json'],
        ]);
        if ($body !== null) {
            curl_setopt($ch, CURLOPT_POSTFIELDS, json_encode($body));
        }
        $raw = curl_exec($ch);
        $status = (int)curl_getinfo($ch, CURLINFO_HTTP_CODE);
        $err = curl_error($ch);
        curl_close($ch);
        if ($raw === false) {
            return sprintf(gettext('Could not reach Breeze Core at %s: %s'), $base, $err);
        }
        return [$status, json_decode((string)$raw, true)];
    }

    private static function failed($detail)
    {
        return ['result' => 'failed', 'message' => $detail];
    }

    private static function detail($status, $body)
    {
        $d = is_array($body) && isset($body['detail']) ? (string)$body['detail'] : '';
        if ($status === 403) {
            return gettext('Breeze Core refused: admin actions need it listening on a private address, and this one is not.') .
                ($d !== '' ? " ({$d})" : '');
        }
        return $d !== '' ? $d : sprintf(gettext('Breeze Core answered %d.'), $status);
    }

    public function listAction()
    {
        $r = $this->call('GET', '/api/auth/devices');
        if (is_string($r)) {
            return self::failed($r);
        }
        [$status, $body] = $r;
        if ($status !== 200 || !is_array($body)) {
            return self::failed(self::detail($status, $body));
        }
        return ['result' => 'ok', 'devices' => $body];
    }

    public function approveAction()
    {
        if (!$this->request->isPost()) {
            return self::failed('POST only');
        }
        $this->throwReadOnly();
        $code = strtoupper(str_replace([' ', '-'], '', trim((string)$this->request->getPost('code'))));
        // The server's alphabet: base32 capitals.
        if (!preg_match('/^[A-Z2-7]{4,16}$/', $code)) {
            return self::failed(gettext('That is not a pairing code: the letters A to Z and the digits 2 to 7, as the app or panel shows them.'));
        }
        $r = $this->call('POST', '/api/auth/enroll/approve', ['code' => $code]);
        if (is_string($r)) {
            return self::failed($r);
        }
        [$status, $body] = $r;
        if ($status !== 200) {
            // 404 covers wrong, expired (a code lives 60 seconds) and used.
            return self::failed($status === 404
                ? gettext('No pending pairing matches that code. Codes last 60 seconds: start pairing again and approve the new one.')
                : self::detail($status, $body));
        }
        return ['result' => 'approved', 'label' => (string)($body['label'] ?? '')];
    }

    public function revokeAction()
    {
        if (!$this->request->isPost()) {
            return self::failed('POST only');
        }
        $this->throwReadOnly();
        $id = (string)$this->request->getPost('token_id');
        if (!preg_match('/^[A-Za-z0-9_-]{1,128}$/', $id)) {
            return self::failed(gettext('Not a device id.'));
        }
        $r = $this->call('DELETE', '/api/auth/devices/' . $id);
        if (is_string($r)) {
            return self::failed($r);
        }
        [$status, $body] = $r;
        if ($status !== 204 && $status !== 200) {
            return self::failed(self::detail($status, $body));
        }
        return ['result' => 'revoked'];
    }
}
