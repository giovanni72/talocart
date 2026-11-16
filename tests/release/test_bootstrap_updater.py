"""Bootstrap input boundaries; actual installation is tested with Ubuntu/systemd."""
import os
from pathlib import Path
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch
import ssl

sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'scripts/release'))
import bootstrap_updater as bootstrap
import update_launcher as launcher
import updates_feed as feed


class BootstrapInputs(unittest.TestCase):
    def test_bounded_inputs_reject_links_wrong_owner_and_nonregular_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            source=root/'source'; source.write_bytes(b'synthetic input')
            self.assertEqual(bootstrap.bounded(source,15,owner=os.getuid()),b'synthetic input')
            for limit,owner in [(3,os.getuid()),(15,os.getuid()+1)]:
                with self.assertRaises(ValueError): bootstrap.bounded(source,limit,owner=owner)
            link=root/'link'; link.symlink_to(source)
            with self.assertRaises(OSError): bootstrap.bounded(link,15)
            fifo=root/'fifo'; os.mkfifo(fifo)
            with self.assertRaises(ValueError): bootstrap.bounded(fifo,15)

    def test_public_key_identity_uses_ed25519_der_instead_of_pem_whitespace(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            private=root/'private'; public=root/'public'; alternate=root/'alternate'
            feed.command(['openssl','genpkey','-algorithm','ED25519','-out',private])
            public.write_bytes(feed.command(['openssl','pkey','-in',private,'-pubout']))
            alternate.write_bytes(b'\n'+public.read_bytes()+b'\n')
            self.assertEqual(bootstrap.public_key(public,SimpleNamespace(command=feed.command)),bootstrap.public_key(alternate,SimpleNamespace(command=feed.command)))
            rsa=root/'rsa'; rsa_public=root/'rsa-public'
            feed.command(['openssl','genpkey','-algorithm','RSA','-pkeyopt','rsa_keygen_bits:1024','-out',rsa])
            rsa_public.write_bytes(feed.command(['openssl','pkey','-in',rsa,'-pubout']))
            with self.assertRaises(ValueError): bootstrap.public_key(rsa_public,SimpleNamespace(command=feed.command))

    def test_server_level_header_does_not_prove_a_location_strips_the_secret(self):
        valid='server { location / { proxy_set_header X-Talocart-Update-Probe ""; proxy_pass http://127.0.0.1; } }'
        bootstrap.proxy_headers(valid)
        for site in [
            'server { proxy_set_header X-Talocart-Update-Probe ""; location / { proxy_set_header Host $host; proxy_pass http://127.0.0.1; } }',
            valid.replace('""','"$http_x_talocart_update_probe"'),
            valid.replace('proxy_set_header X-Talocart-Update-Probe "";', '# proxy_set_header X-Talocart-Update-Probe "";\n'),
            valid.replace('proxy_pass','include proxy_params; proxy_pass'),
        ]:
            with self.subTest(site=site), self.assertRaises(ValueError): bootstrap.proxy_headers(site)
        bootstrap.proxy_headers(valid.replace('""', "''"))

    def test_gateway_reload_waits_only_for_new_listener_and_has_fixed_deadline(self):
        probe=SimpleNamespace(check=Mock(side_effect=[ConnectionRefusedError(),None]))
        with patch('time.sleep') as sleep:
            bootstrap.gateway_ready(probe)
        self.assertEqual(probe.check.call_count,2)
        sleep.assert_called_once_with(0.05)
        for error in (ssl.SSLCertVerificationError(),ValueError('Invalid gateway'),TimeoutError()):
            probe.check=Mock(side_effect=error)
            with self.subTest(error=type(error).__name__),patch('time.sleep') as sleep:
                with self.assertRaises(type(error)): bootstrap.gateway_ready(probe)
                sleep.assert_not_called()
        probe.check=Mock(side_effect=ConnectionRefusedError())
        with patch('time.monotonic',side_effect=[0,5]),patch('time.sleep') as sleep:
            with self.assertRaises(ConnectionRefusedError): bootstrap.gateway_ready(probe)
            sleep.assert_not_called()

    def test_units_use_fixed_launcher_and_never_execute_application_package_as_root(self):
        self.assertIn('ExecStart=/usr/bin/python3 -I /usr/local/libexec/talocart/update_launcher.py',bootstrap.SERVICE_TEXT)
        self.assertIn('RuntimeDirectoryMode=0700',bootstrap.SERVICE_TEXT)
        self.assertIn('PathExists=/var/lib/talocart-update/requests/install.json',bootstrap.PATH_TEXT)
        self.assertIn('PathExists=/var/lib/talocart-update/requests/check.json',bootstrap.PATH_TEXT)
        self.assertNotIn('/opt/talocart/current',bootstrap.SERVICE_TEXT)


if __name__=='__main__': unittest.main()
