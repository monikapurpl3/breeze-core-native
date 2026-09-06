<?php

namespace OPNsense\BreezeCore;

use OPNsense\Base\IndexController as BaseIndexController;

class IndexController extends BaseIndexController
{
    public function indexAction()
    {
        $this->view->title = "Breeze Core";
        $this->view->formGeneral = $this->getForm("general");
        $this->view->pick("OPNsense/BreezeCore/index");
    }
}
