##############################################################################
# Loadgen Worker Nodes
#
# Each node runs one worker-agent (Deno + FFI) that receives benchmark
# instructions from the distributed controller.
##############################################################################

resource "hcloud_server" "worker" {
  count = var.worker_count

  name        = "${var.label_prefix}-worker-${count.index}"
  server_type = var.worker_type
  image       = var.image
  location    = var.location
  ssh_keys    = [data.hcloud_ssh_key.loadgen.id]

  labels = {
    project = var.label_prefix
    role    = "worker"
  }

  network {
    network_id = hcloud_network.loadgen.id
    ip         = "10.0.1.${10 + count.index}"
  }

  depends_on = [hcloud_network_subnet.loadgen]
}
