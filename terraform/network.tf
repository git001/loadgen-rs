##############################################################################
# Network – Private network for benchmark traffic
#
# Workers communicate with the controller over the public IP,
# but the private network can be used for worker-to-target traffic
# if the benchmark target is in the same network.
##############################################################################

resource "hcloud_network" "loadgen" {
  name     = "${var.label_prefix}-network"
  ip_range = "10.0.0.0/16"
}

resource "hcloud_network_subnet" "loadgen" {
  network_id   = hcloud_network.loadgen.id
  type         = "cloud"
  network_zone = "eu-central"
  ip_range     = "10.0.1.0/24"
}

data "hcloud_ssh_key" "loadgen" {
  name = var.ssh_key_name
}
