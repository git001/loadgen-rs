##############################################################################
# Firewall – Allow SSH + worker-agent port from anywhere
#
# Adjust source_ips to restrict access to your IP for production use.
##############################################################################

resource "hcloud_firewall" "worker" {
  name = "${var.label_prefix}-worker-fw"

  rule {
    description = "Allow SSH"
    direction   = "in"
    protocol    = "tcp"
    port        = "22"
    source_ips  = ["0.0.0.0/0", "::/0"]
  }

  rule {
    description = "Allow worker-agent port"
    direction   = "in"
    protocol    = "tcp"
    port        = tostring(var.worker_port)
    source_ips  = ["0.0.0.0/0", "::/0"]
  }
}

resource "hcloud_firewall_attachment" "worker" {
  firewall_id = hcloud_firewall.worker.id
  server_ids  = hcloud_server.worker[*].id
}
