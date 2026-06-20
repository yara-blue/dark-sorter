machine.copy_from_host("test-assets/RAW_NIKON_D1.NEF", "/source")
machine.copy_from_host("test-assets/RAW_NIKON_D1.NEF.xmp", "/source")
machine.wait_for_unit("default.target")
machine.wait_for_file("/target/RAW_NIKON_D1.jpeg", 1)
