# @summary Manages the installation, configuration, and service of the Proxmox VE SAN fencing daemon.
#
# @param poll_interval
#   Interval in seconds between multipathd checks.
# @param max_failures
#   Number of consecutive failures before fencing is triggered.
# @param discovery_interval
#   Interval in seconds between VM and storage discovery scans.
# @param socket
#   Multipathd socket path (e.g. '@/org/kernel/linux/storage/multipathd').
# @param sysrq_char
#   Comma-separated list of SysRq characters to send sequentially on fence (e.g. 's,b').
# @param test_mode
#   Set to true to run in test mode (fencing decisions are logged but trigger_fencing
#   is never called and the daemon stays running).
# @param fence_dry_run
#   Set to a non-empty value to call trigger_fencing on a fencing decision, but instead
#   of writing to SysRq the daemon logs the decision, flushes the status file, and exits
#   with code 0. Unlike test_mode, the daemon does not continue running. Intended for
#   one-shot integration tests.
# @param debug
#   Set to true to enable verbose debug logging of discovered VMs/storages.
# @param discovery_max_retries
#   Maximum consecutive discovery failures before applying exponential backoff (0 = no backoff).
# @param discovery_backoff_base
#   Base delay in seconds for exponential backoff.
# @param discovery_backoff_max
#   Maximum backoff delay in seconds.
# @param fence_reboot_timeout
#   Seconds to wait after sending the reboot SysRq character before retrying.
# @param max_response_size
#   Maximum size in bytes of a multipathd JSON response.
# @param package_name
#   The name of the package to install.
# @param service_name
#   The name of the systemd service to manage.
class pve_san_fenced (
  Integer         $poll_interval           = 5,
  Integer         $max_failures            = 6,
  Integer         $discovery_interval      = 60,
  String          $socket                  = '@/org/kernel/linux/storage/multipathd',
  String          $sysrq_char              = 's,b',
  Boolean         $test_mode               = false,
  Optional[String] $fence_dry_run          = undef,
  Boolean         $debug                   = false,
  Integer         $discovery_max_retries   = 5,
  Integer         $discovery_backoff_base  = 1,
  Integer         $discovery_backoff_max   = 30,
  Integer         $fence_reboot_timeout    = 10,
  Integer         $max_response_size       = 104857600,
  String          $package_name            = 'pve-san-fenced',
  String          $service_name            = 'pve-san-fenced',
) {
  package { $package_name:
    ensure => installed,
  }

  $config_content = @("CONFIG")
    # Configuration for pve-san-fenced daemon

    # Poll interval in seconds
    PVE_SAN_POLL_INTERVAL=${poll_interval}

    # Maximum consecutive failures before fencing
    PVE_SAN_MAX_FAILURES=${max_failures}

    # Discovery interval in seconds
    PVE_SAN_DISCOVERY_INTERVAL=${discovery_interval}

    # Multipathd socket path
    PVE_SAN_SOCKET=${socket}

    # SysRq character to trigger fencing (default is 's,b' for sync and reboot)
    PVE_SAN_SYSRQ_CHAR=${sysrq_char}

    # Set to true to run in test mode. Fencing decisions are logged but trigger_fencing
    # is never called and the daemon stays running.
    PVE_SAN_TEST_MODE=${test_mode}

    # If set, trigger_fencing is called on a fencing decision, but instead of writing
    # to SysRq the daemon logs the decision, flushes the status file, and exits with
    # code 0. Unlike PVE_SAN_TEST_MODE, the daemon does not continue running.
    # Intended for one-shot integration tests.
    ${fence_dry_run ? {
      undef   => '# PVE_SAN_FENCE_DRY_RUN=',
      default => "PVE_SAN_FENCE_DRY_RUN=${fence_dry_run}"
    }}

    # Set to true to enable verbose debug logging of discovered VMs, storages, and multipaths on each discovery run
    PVE_SAN_DEBUG=${debug}

    # Maximum consecutive discovery failures before applying exponential backoff (0 = no backoff)
    PVE_SAN_DISCOVERY_MAX_RETRIES=${discovery_max_retries}

    # Base delay in seconds for exponential backoff
    PVE_SAN_DISCOVERY_BACKOFF_BASE=${discovery_backoff_base}

    # Maximum backoff delay in seconds
    PVE_SAN_DISCOVERY_BACKOFF_MAX=${discovery_backoff_max}

    # Seconds to wait after sending the reboot SysRq character before retrying
    PVE_SAN_FENCE_REBOOT_TIMEOUT=${fence_reboot_timeout}

    # Maximum size in bytes of a multipathd JSON response
    PVE_SAN_MAX_RESPONSE_SIZE=${max_response_size}
    |-CONFIG

  file { '/etc/default/pve-san-fenced':
    ensure  => file,
    owner   => 'root',
    group   => 'root',
    mode    => '0644',
    content => $config_content,
    require => Package[$package_name],
    notify  => Service[$service_name],
  }

  service { $service_name:
    ensure    => running,
    enable    => true,
    subscribe => File['/etc/default/pve-san-fenced'],
  }
}
