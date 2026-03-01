##############################################################################
# Outputs – Used by generate-inventory.sh
##############################################################################

output "worker_public_ips" {
  description = "Public IPv4 addresses of the worker nodes"
  value       = hcloud_server.worker[*].ipv4_address
}

output "worker_private_ips" {
  description = "Private (network) IPs of the worker nodes"
  value       = [for i in range(var.worker_count) : "10.0.1.${10 + i}"]
}

output "worker_names" {
  description = "Names of the worker nodes"
  value       = hcloud_server.worker[*].name
}

output "worker_port" {
  description = "Worker-agent port"
  value       = var.worker_port
}

output "deploy_dir" {
  description = "Remote deployment directory"
  value       = var.deploy_dir
}

output "deno_version" {
  description = "Deno version configured"
  value       = var.deno_version
}

output "ssh_summary" {
  description = "Quick SSH commands for each worker"
  value = [for i, inst in hcloud_server.worker :
    "ssh root@${inst.ipv4_address}  # ${inst.name}"
  ]
}
