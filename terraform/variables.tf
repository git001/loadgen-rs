##############################################################################
# Hetzner Cloud API Token
##############################################################################
variable "hcloud_token" {
  description = "Hetzner Cloud API Token (export TF_VAR_hcloud_token=...)"
  type        = string
  sensitive   = true
}

##############################################################################
# Location & Sizing
##############################################################################
variable "location" {
  description = "Hetzner Cloud location for all resources"
  type        = string
  default     = "fsn1" # Falkenstein
}

variable "worker_type" {
  description = "Hetzner server type for worker nodes (Dedicated vCPU recommended)"
  type        = string
  default     = "ccx23" # 8 vCPU, 16 GB RAM (Dedicated)
}

variable "image" {
  description = "OS image for all instances"
  type        = string
  default     = "ubuntu-24.04"
}

##############################################################################
# Counts
##############################################################################
variable "worker_count" {
  description = "Number of loadgen worker nodes"
  type        = number
  default     = 2
}

##############################################################################
# SSH Key
##############################################################################
variable "ssh_key_name" {
  description = "Name of an existing Hetzner Cloud SSH key to deploy on instances"
  type        = string
}

##############################################################################
# Worker Settings
##############################################################################
variable "worker_port" {
  description = "Port the worker-agent listens on"
  type        = number
  default     = 9091
}

variable "deploy_dir" {
  description = "Remote directory for loadgen deployment"
  type        = string
  default     = "/opt/loadgen"
}

variable "deno_version" {
  description = "Deno version to install on workers"
  type        = string
  default     = "2.7.1"
}

##############################################################################
# Labels
##############################################################################
variable "label_prefix" {
  description = "Prefix for resource names and labels"
  type        = string
  default     = "loadgen"
}
